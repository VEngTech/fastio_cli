//! Workspace service account (`fastio service-account`, alias `sa`) handlers.
//!
//! Surface over [`fastio_cli::api::service_account`]. Management runs with a
//! workspace admin's or owner's own credential; the server refuses it for a
//! service account key. The disciplines this module enforces:
//!
//! - **The secret is shown once, in every format.** `create` and `key create`
//!   are the only calls that ever return a key's secret. Their responses are
//!   reshaped into ONE flat object with `secret` as its last field, because the
//!   raw `{service_account, key}` shape loses the key (and its secret) in the
//!   table / CSV flattening. The table prints the secret on its own line after
//!   the table instead, so its dynamic layout cannot wrap it mid-token.
//!   `--quiet` and `--fields` would also discard it, so both are refused
//!   BEFORE any request is sent.
//! - **Mints are never replayed.** Both go out exactly once and never follow a
//!   redirect. When the outcome is unknown (transport failure, 5xx, redirect,
//!   unreadable success body) the error tells the user how to find out
//!   whether the account or key exists and how to replace a key whose secret
//!   was lost — never to simply re-run.
//! - **Other shapes are flattened for every format**, so table / CSV do not
//!   silently drop the plan limit on `list` or the account on `info`.
//! - **Destructive actions confirm first** (`delete`, `key revoke`) through the
//!   shared [`confirm_destructive`] gate, before any request.

use std::collections::HashSet;
use std::io::Write as _;

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};

use fastio_cli::api::service_account::{self as sa_api, CreateParams, MintedKey};
use fastio_cli::error::CliError;
use fastio_cli::output::{OutputConfig, OutputFormat, csv_output, table};

use crate::cli::{ServiceAccountCommands, ServiceAccountKeyCommands};

use super::CommandContext;
use super::confirm::confirm_destructive;

/// Printed to stderr after a secret has been written to stdout.
const SECRET_NOTICE: &str = "The new key's secret was written to stdout. Store it now: it is shown only once and cannot be retrieved again. \
     Use it as this service account's API key (for example `FASTIO_API_KEY=<secret> fastio …`).";

/// Execute a `service-account` subcommand.
pub async fn execute(command: ServiceAccountCommands, ctx: &CommandContext<'_>) -> Result<()> {
    match command {
        ServiceAccountCommands::List { workspace } => list(ctx, &workspace).await,
        ServiceAccountCommands::Create {
            workspace,
            name,
            role,
            key_expires,
            key_memo,
        } => {
            create(
                ctx,
                &workspace,
                &CreateParams {
                    name: &name,
                    role: &role,
                    key_expires: key_expires.as_deref(),
                    key_memo: key_memo.as_deref(),
                },
            )
            .await
        }
        ServiceAccountCommands::Info {
            service_account_id,
            workspace,
        } => info(ctx, &workspace, &service_account_id).await,
        ServiceAccountCommands::Update {
            service_account_id,
            workspace,
            name,
            role,
        } => {
            let client = ctx.build_client()?;
            let value = sa_api::update(
                &client,
                &workspace,
                &service_account_id,
                name.as_deref(),
                role.as_deref(),
            )
            .await
            .context("failed to update service account")?;
            ctx.output.render(&value)?;
            Ok(())
        }
        ServiceAccountCommands::Disable {
            service_account_id,
            workspace,
        } => {
            let client = ctx.build_client()?;
            let value = sa_api::disable(&client, &workspace, &service_account_id)
                .await
                .context("failed to disable service account")?;
            ctx.output.render(&value)?;
            Ok(())
        }
        ServiceAccountCommands::Enable {
            service_account_id,
            workspace,
        } => {
            let client = ctx.build_client()?;
            let value = sa_api::enable(&client, &workspace, &service_account_id)
                .await
                .context("failed to enable service account")?;
            ctx.output.render(&value)?;
            Ok(())
        }
        ServiceAccountCommands::Delete {
            service_account_id,
            workspace,
            yes,
        } => delete(ctx, &workspace, &service_account_id, yes).await,
        ServiceAccountCommands::Key(key) => execute_key(key, ctx).await,
    }
}

/// Execute a `service-account key` subcommand.
async fn execute_key(command: ServiceAccountKeyCommands, ctx: &CommandContext<'_>) -> Result<()> {
    match command {
        ServiceAccountKeyCommands::List {
            service_account_id,
            workspace,
        } => {
            let client = ctx.build_client()?;
            let value = sa_api::keys_list(&client, &workspace, &service_account_id)
                .await
                .context("failed to list service account keys")?;
            ctx.output.render(&value)?;
            Ok(())
        }
        ServiceAccountKeyCommands::Create {
            service_account_id,
            workspace,
            expires,
            memo,
        } => {
            key_create(
                ctx,
                &workspace,
                &service_account_id,
                expires.as_deref(),
                memo.as_deref(),
            )
            .await
        }
        ServiceAccountKeyCommands::Revoke {
            service_account_id,
            key_id,
            workspace,
            yes,
        } => key_revoke(ctx, &workspace, &service_account_id, &key_id, yes).await,
    }
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// `service-account list`.
async fn list(ctx: &CommandContext<'_>, workspace: &str) -> Result<()> {
    let client = ctx.build_client()?;
    let value = sa_api::list(&client, workspace)
        .await
        .context("failed to list service accounts")?;
    ctx.output.render(&list_value(&value))?;
    Ok(())
}

/// `service-account info`.
async fn info(ctx: &CommandContext<'_>, workspace: &str, service_account_id: &str) -> Result<()> {
    let client = ctx.build_client()?;
    let value = sa_api::details(&client, workspace, service_account_id)
        .await
        .context("failed to get service account details")?;
    ctx.output.render(&info_value(&value))?;
    Ok(())
}

/// `service-account create`: one request, the secret rendered once.
async fn create(
    ctx: &CommandContext<'_>,
    workspace: &str,
    params: &CreateParams<'_>,
) -> Result<()> {
    refuse_secret_suppressing_output(ctx.output, "service-account create")?;
    sa_api::require_id(workspace, "workspace ID")?;

    let client = ctx.build_client()?;
    let created = match sa_api::create(&client, workspace, params).await {
        Ok(created) => created,
        Err(err) if mint_outcome_unknown(&err) => {
            return Err(anyhow::Error::from(err)
                .context(create_outcome_unknown_message(workspace, params.name)));
        }
        Err(err) => {
            return Err(anyhow::Error::from(err).context("failed to create service account"));
        }
    };

    let (mut value, service_account_id) = match &created.service_account {
        Some(account) => (
            flatten_with_rename(account, "", Some("service_account_id")),
            account
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("<service-account-id>")
                .to_owned(),
        ),
        None => (Map::new(), "<service-account-id>".to_owned()),
    };
    append_minted_key(&mut value, &created.key);
    render_secret_once(
        ctx.output,
        value,
        created.key.expose_secret(),
        workspace,
        &service_account_id,
        created.key.key_id(),
    )?;
    if created.service_account.is_none() {
        eprintln!(
            "warning: the response did not include the new service account's details. The \
             account was created and the key above belongs to it; find its id with \
             `fastio service-account list --workspace {workspace}`."
        );
    }
    Ok(())
}

/// `service-account key create`: one request, the secret rendered once.
async fn key_create(
    ctx: &CommandContext<'_>,
    workspace: &str,
    service_account_id: &str,
    expires: Option<&str>,
    memo: Option<&str>,
) -> Result<()> {
    refuse_secret_suppressing_output(ctx.output, "service-account key create")?;
    sa_api::require_id(workspace, "workspace ID")?;
    sa_api::require_id(service_account_id, "service account ID")?;

    let client = ctx.build_client()?;
    let minted = match sa_api::key_create(&client, workspace, service_account_id, expires, memo)
        .await
    {
        Ok(minted) => minted,
        Err(err) if mint_outcome_unknown(&err) => {
            return Err(
                anyhow::Error::from(err).context(key_create_outcome_unknown_message(
                    workspace,
                    service_account_id,
                )),
            );
        }
        Err(err) => {
            return Err(anyhow::Error::from(err).context("failed to create service account key"));
        }
    };

    let mut value = Map::new();
    value.insert("service_account_id".to_owned(), json!(service_account_id));
    append_minted_key(&mut value, &minted);
    render_secret_once(
        ctx.output,
        value,
        minted.expose_secret(),
        workspace,
        service_account_id,
        minted.key_id(),
    )
}

/// `service-account delete`: confirm, then delete.
async fn delete(
    ctx: &CommandContext<'_>,
    workspace: &str,
    service_account_id: &str,
    yes: bool,
) -> Result<()> {
    sa_api::require_id(workspace, "workspace ID")?;
    sa_api::require_id(service_account_id, "service account ID")?;
    confirm_destructive(
        "service-account delete",
        &format!(
            "permanently closes service account {service_account_id} and deletes all of its keys \
             (use `fastio service-account disable` for a reversible stop)"
        ),
        yes,
    )?;
    let client = ctx.build_client()?;
    sa_api::delete(&client, workspace, service_account_id)
        .await
        .context("failed to delete service account")?;
    ctx.output.render(&json!({
        "status": "deleted",
        "service_account_id": service_account_id,
    }))?;
    Ok(())
}

/// `service-account key revoke`: confirm, then revoke.
async fn key_revoke(
    ctx: &CommandContext<'_>,
    workspace: &str,
    service_account_id: &str,
    key_id: &str,
    yes: bool,
) -> Result<()> {
    sa_api::require_id(workspace, "workspace ID")?;
    sa_api::require_id(service_account_id, "service account ID")?;
    sa_api::require_id(key_id, "key ID")?;
    confirm_destructive(
        "service-account key revoke",
        &format!(
            "permanently revokes key {key_id} of service account {service_account_id}: its secret \
             stops working at once, and a new key comes with a new secret"
        ),
        yes,
    )?;
    let client = ctx.build_client()?;
    sa_api::key_revoke(&client, workspace, service_account_id, key_id)
        .await
        .context("failed to revoke service account key")?;
    ctx.output.render(&json!({
        "status": "revoked",
        "service_account_id": service_account_id,
        "key_id": key_id,
    }))?;
    Ok(())
}

// ─── Secret handling ─────────────────────────────────────────────────────────

/// Refuse output flags that would discard a one-time secret.
///
/// Runs before authentication and before any request: once the server has
/// minted the key it is too late, because the secret cannot be fetched again.
fn refuse_secret_suppressing_output(output: &OutputConfig, command: &str) -> Result<()> {
    if output.quiet {
        anyhow::bail!(
            "`{command}` prints the new key's secret once, and it can never be retrieved again, \
             so --quiet is not allowed here (it would discard the secret). Re-run without --quiet; \
             to capture the output, redirect stdout to a file only you can read."
        );
    }
    if output.fields.is_some() {
        anyhow::bail!(
            "`{command}` prints the new key's secret once, and it can never be retrieved again, \
             so --fields is not allowed here (it could filter the secret out). Re-run without --fields."
        );
    }
    Ok(())
}

/// Whether a failed mint may nonetheless have been committed server-side.
///
/// A transport failure, a 5xx (a gateway may answer after the server already
/// acted), an unfollowed redirect, or a success body that could not be read or
/// parsed (the last two surface as [`CliError::Parse`]) all leave the outcome
/// unknown. A 4xx is the server's definite refusal.
fn mint_outcome_unknown(err: &CliError) -> bool {
    match err {
        CliError::Http(_) | CliError::Parse(_) => true,
        CliError::Api(api) => api.http_status >= 500,
        _ => false,
    }
}

/// Recovery guidance when a `create` may have succeeded but its response
/// (and therefore the first key's secret) was lost.
fn create_outcome_unknown_message(workspace: &str, name: &str) -> String {
    format!(
        "the service account may have been created even though this command failed: the response \
         was lost, and it carried the first key's secret, which cannot be retrieved again. Do not \
         simply re-run this command. Check with `fastio service-account list --workspace {workspace}`; \
         if \"{name}\" is listed, create a replacement key with `fastio service-account key create \
         <service-account-id> --workspace {workspace}` and revoke the unusable one with \
         `fastio service-account key revoke <service-account-id> <key-id> --workspace {workspace}`"
    )
}

/// Recovery guidance when a `key create` may have succeeded but its response
/// (and therefore the secret) was lost.
fn key_create_outcome_unknown_message(workspace: &str, service_account_id: &str) -> String {
    format!(
        "a key may have been created even though this command failed: the response was lost, and \
         it carried the secret, which cannot be retrieved again. Check with `fastio service-account \
         key list {service_account_id} --workspace {workspace}`; revoke any key whose secret you do \
         not hold with `fastio service-account key revoke {service_account_id} <key-id> --workspace \
         {workspace}`, then create a new one"
    )
}

/// Render a payload carrying a one-time secret, then print the stderr notice.
///
/// JSON / CSV / markdown get the flat object with `secret` as its LAST field
/// (CSV goes straight to the CSV writer, skipping the generic flattening).
/// The table renders the object WITHOUT the secret and then prints the secret
/// on its own `secret: …` line: the table's dynamic layout wraps long cells to
/// the terminal width, which would split the secret mid-token.
///
/// A render failure must not lose the secret silently: the error says the key
/// cannot be recovered and how to replace it. It never embeds the payload.
fn render_secret_once(
    output: &OutputConfig,
    value: Map<String, Value>,
    secret: &str,
    workspace: &str,
    service_account_id: &str,
    key_id: Option<&str>,
) -> Result<()> {
    let failure = || {
        format!(
            "failed to print the new key's secret, and it cannot be retrieved again. Revoke key {} \
             with `fastio service-account key revoke {service_account_id} {} --workspace {workspace}` \
             and create a new one",
            key_id.unwrap_or("<key-id>"),
            key_id.unwrap_or("<key-id>"),
        )
    };
    match output.format {
        OutputFormat::Table => {
            let text = table_with_secret_line(&value, secret, output.no_color, None);
            std::io::stdout()
                .lock()
                .write_all(text.as_bytes())
                .with_context(failure)?;
        }
        // Straight to the CSV writer: the generic path flattens first, and a
        // future array field would then win and drop the secret.
        OutputFormat::Csv => {
            csv_output::render(&Value::Object(with_secret_last(value, secret)))
                .with_context(failure)?;
        }
        _ => {
            output
                .render(&Value::Object(with_secret_last(value, secret)))
                .with_context(failure)?;
        }
    }
    eprintln!("{SECRET_NOTICE}");
    Ok(())
}

/// `value` with the secret as its LAST field — after every server field, so a
/// same-named server field can neither replace nor duplicate it.
fn with_secret_last(mut value: Map<String, Value>, secret: &str) -> Map<String, Value> {
    value.shift_remove("secret");
    value.insert("secret".to_owned(), Value::String(secret.to_owned()));
    value
}

/// The table form of a secret-bearing payload: the table without the secret,
/// then the secret on its own unwrapped line. `width` pins the table layout
/// (tests); `None` uses the terminal's.
fn table_with_secret_line(
    value: &Map<String, Value>,
    secret: &str,
    no_color: bool,
    width: Option<u16>,
) -> String {
    let mut rows = value.clone();
    rows.shift_remove("secret");
    format!(
        "{}\n{}\n",
        table::to_text(&Value::Object(rows), no_color, width),
        table::labelled_line("secret", secret)
    )
}

/// Append a minted key's metadata as `key_*` fields. The secret is added
/// separately, by [`render_secret_once`].
fn append_minted_key(dst: &mut Map<String, Value>, minted: &MintedKey) {
    for (name, value) in flatten_fields(&minted.key) {
        dst.insert(format!("key_{name}"), value);
    }
}

// ─── Presentation (pure; unit-tested) ────────────────────────────────────────

/// Flatten one level of nesting: `created_by: {id, name}` becomes
/// `created_by_id` / `created_by_name`. Scalars and arrays pass through.
///
/// Every row these commands render is then a flat object, which table, CSV,
/// markdown, and JSON all present without dropping a field.
fn flatten_fields(src: &Map<String, Value>) -> Vec<(String, Value)> {
    src.iter()
        .flat_map(|(name, value)| flatten_entry(name, value))
        .collect()
}

/// [`flatten_fields`] for one top-level field.
fn flatten_entry(name: &str, value: &Value) -> Vec<(String, Value)> {
    match value {
        Value::Object(inner) => inner
            .iter()
            .map(|(inner_name, inner_value)| (format!("{name}_{inner_name}"), inner_value.clone()))
            .collect(),
        other => vec![(name.to_owned(), other.clone())],
    }
}

/// [`flatten_fields`] into a map, renaming `id` when `rename_id` is set.
fn flatten_with_rename(
    src: &Map<String, Value>,
    prefix: &str,
    rename_id: Option<&str>,
) -> Map<String, Value> {
    let mut out = Map::new();
    for (name, value) in flatten_fields(src) {
        let name = match rename_id {
            Some(renamed) if name == "id" => renamed.to_owned(),
            _ => format!("{prefix}{name}"),
        };
        out.insert(name, value);
    }
    out
}

/// `list`: one row per service account with the plan limit stamped on each
/// (`limit_max` / `limit_used`). An empty list still reports the limit, as a
/// single summary object.
fn list_value(response: &Value) -> Value {
    let limit = response.get("limit");
    let limit_max = limit.and_then(|l| l.get("max")).cloned();
    let limit_used = limit.and_then(|l| l.get("used")).cloned();
    let stamp = |row: &mut Map<String, Value>| {
        if let Some(v) = &limit_max {
            row.insert("limit_max".to_owned(), v.clone());
        }
        if let Some(v) = &limit_used {
            row.insert("limit_used".to_owned(), v.clone());
        }
    };

    match response.get("service_accounts").and_then(Value::as_array) {
        Some(items) if !items.is_empty() => Value::Array(
            items
                .iter()
                .map(|item| match item {
                    Value::Object(account) => {
                        let mut row = flatten_with_rename(account, "", None);
                        stamp(&mut row);
                        Value::Object(row)
                    }
                    other => other.clone(),
                })
                .collect(),
        ),
        Some(_) if limit_max.is_some() || limit_used.is_some() => {
            let mut summary = Map::new();
            summary.insert("service_accounts".to_owned(), json!(0));
            stamp(&mut summary);
            Value::Object(summary)
        }
        Some(_) => Value::Array(Vec::new()),
        // Not the documented shape: render what the server sent.
        None => response.clone(),
    }
}

/// `info`: one row per key with the account's fields stamped on (an account
/// field whose name a key already uses is prefixed `account_`). With no keys,
/// the account itself.
fn info_value(response: &Value) -> Value {
    let Some(account) = response.get("service_account").and_then(Value::as_object) else {
        return response.clone();
    };
    let raw_keys: Vec<&Map<String, Value>> = response
        .get("keys")
        .and_then(Value::as_array)
        .map(|keys| keys.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default();

    if raw_keys.is_empty() {
        return Value::Object(flatten_fields(account).into_iter().collect());
    }
    let keys: Vec<Map<String, Value>> = raw_keys
        .iter()
        .map(|key| flatten_with_rename(key, "", None))
        .collect();

    // Decide collisions on the RAW top-level names, over ALL key rows, BEFORE
    // flattening: `created_by` may be an object on one side and null on the
    // other, and comparing flattened names would then leave the account's
    // creator unprefixed, reading as the key's creator.
    let key_names: HashSet<&str> = raw_keys
        .iter()
        .flat_map(|k| k.keys())
        .map(String::as_str)
        .collect();
    let stamped: Vec<(String, Value)> = account
        .iter()
        .flat_map(|(name, value)| {
            let collides = key_names.contains(name.as_str());
            flatten_entry(name, value)
                .into_iter()
                .map(move |(flat, v)| {
                    if collides {
                        (format!("account_{flat}"), v)
                    } else {
                        (flat, v)
                    }
                })
        })
        .collect();

    Value::Array(
        keys.iter()
            .map(|key| {
                let mut row = key.clone();
                for (name, value) in &stamped {
                    row.insert(name.clone(), value.clone());
                }
                Value::Object(row)
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastio_cli::error::ApiError;

    const SA_ID: &str = "9876543210987654321";

    fn account() -> Value {
        json!({
            "id": SA_ID,
            "workspace_id": "1234567890123456789",
            "name": "ci-bot",
            "role": "editor",
            "status": "active",
            "created": "2026-10-16 12:00:00 UTC",
            "updated": "2026-10-16 12:00:00 UTC",
            "created_by": {"id": "1111111111111111111", "name": "Admin"},
            "keys_active": 1,
            "next_key_expiry": null,
            "last_used": null
        })
    }

    fn key(id: &str) -> Value {
        json!({
            "id": id,
            "hint": "fio_sa_…abcd",
            "memo": "",
            "created": "2026-10-16 12:00:00 UTC",
            "created_by": {"id": "1111111111111111111", "name": "Admin"},
            "expires": null,
            "expired": false,
            "last_used": null
        })
    }

    #[test]
    fn quiet_and_fields_are_refused_for_secret_minting_commands() {
        let quiet = OutputConfig::from_flags(Some("json"), None, true, true);
        let err = refuse_secret_suppressing_output(&quiet, "service-account create").unwrap_err();
        assert!(err.to_string().contains("--quiet"), "{err}");
        assert!(err.to_string().contains("once"), "{err}");

        let fields = OutputConfig::from_flags(Some("json"), Some("name"), true, false);
        let err =
            refuse_secret_suppressing_output(&fields, "service-account key create").unwrap_err();
        assert!(err.to_string().contains("--fields"), "{err}");

        let plain = OutputConfig::from_flags(Some("table"), None, true, false);
        assert!(refuse_secret_suppressing_output(&plain, "x").is_ok());
    }

    #[test]
    fn mint_outcome_is_unknown_only_for_transport_5xx_and_unreadable_bodies() {
        assert!(mint_outcome_unknown(&CliError::Parse(
            "bad body".to_owned()
        )));
        assert!(mint_outcome_unknown(&CliError::Api(ApiError::new(
            0,
            None,
            "Bad Gateway".to_owned(),
            502
        ))));
        assert!(mint_outcome_unknown(&CliError::Api(ApiError::new(
            0,
            None,
            "x".to_owned(),
            500
        ))));
        // A 4xx is a definite refusal: nothing was minted.
        for status in [400, 403, 404, 409] {
            assert!(!mint_outcome_unknown(&CliError::Api(ApiError::new(
                1,
                None,
                "x".to_owned(),
                status
            ))));
        }
        assert!(!mint_outcome_unknown(&CliError::RateLimit {
            retry_after_secs: 1
        }));
    }

    #[test]
    fn outcome_unknown_messages_say_how_to_recover_without_rerunning() {
        let msg = create_outcome_unknown_message("123", "ci-bot");
        assert!(msg.contains("may have been created"), "{msg}");
        assert!(
            msg.contains("service-account list --workspace 123"),
            "{msg}"
        );
        assert!(msg.contains("key create"), "{msg}");
        assert!(msg.contains("key revoke"), "{msg}");
        assert!(msg.contains("Do not"), "{msg}");

        let msg = key_create_outcome_unknown_message("123", SA_ID);
        assert!(msg.contains("may have been created"), "{msg}");
        assert!(
            msg.contains(&format!("key list {SA_ID} --workspace 123")),
            "{msg}"
        );
        assert!(msg.contains("key revoke"), "{msg}");
    }

    #[test]
    fn flatten_fields_lifts_one_level_of_nesting() {
        let Value::Object(acct) = account() else {
            unreachable!()
        };
        let flat: Map<String, Value> = flatten_fields(&acct).into_iter().collect();
        assert_eq!(flat["created_by_name"], "Admin");
        assert_eq!(flat["created_by_id"], "1111111111111111111");
        assert!(!flat.contains_key("created_by"));
        assert!(flat.values().all(|v| !v.is_object()));
    }

    #[test]
    fn list_stamps_the_limit_on_every_row() {
        let v = list_value(&json!({
            "service_accounts": [account(), account()],
            "limit": {"max": 5, "used": 2}
        }));
        let rows = v.as_array().expect("rows");
        assert_eq!(rows.len(), 2);
        for row in rows {
            assert_eq!(row["limit_max"], 5);
            assert_eq!(row["limit_used"], 2);
            assert_eq!(row["name"], "ci-bot");
            assert_eq!(row["id"], SA_ID);
        }
    }

    #[test]
    fn empty_list_still_reports_the_limit() {
        let v = list_value(&json!({"service_accounts": [], "limit": {"max": 5, "used": 0}}));
        assert_eq!(
            v,
            json!({"service_accounts": 0, "limit_max": 5, "limit_used": 0})
        );
        // With no limit at all, an empty list stays an empty list.
        assert_eq!(list_value(&json!({"service_accounts": []})), json!([]));
    }

    #[test]
    fn info_with_no_keys_renders_the_account() {
        let v = info_value(&json!({"service_account": account(), "keys": []}));
        assert_eq!(v["name"], "ci-bot");
        assert_eq!(v["id"], SA_ID);
        assert_eq!(v["created_by_name"], "Admin");
    }

    #[test]
    fn info_with_keys_stamps_the_account_on_each_key_row() {
        let v = info_value(&json!({"service_account": account(), "keys": [key("k1"), key("k2")]}));
        let rows = v.as_array().expect("rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["id"], "k1");
        assert_eq!(rows[1]["id"], "k2");
        for row in rows {
            // Colliding account fields are prefixed; the key keeps its own.
            assert_eq!(row["account_id"], SA_ID);
            assert_eq!(row["account_created_by_name"], "Admin");
            assert!(row.get("account_last_used").is_some());
            // Non-colliding account fields keep their names.
            assert_eq!(row["name"], "ci-bot");
            assert_eq!(row["role"], "editor");
            assert_eq!(row["status"], "active");
            assert_eq!(row["hint"], "fio_sa_…abcd");
        }
    }

    #[test]
    fn unexpected_shapes_pass_through_unchanged() {
        let odd = json!({"something": "else"});
        assert_eq!(list_value(&odd), odd);
        assert_eq!(info_value(&odd), odd);
    }

    /// `created_by` null on the KEY, an object on the account: the account's
    /// creator must not appear unprefixed (it would read as the key's creator).
    #[test]
    fn info_null_key_creator_does_not_inherit_the_account_creator() {
        let mut k = key("k1");
        k["created_by"] = Value::Null;
        let v = info_value(&json!({"service_account": account(), "keys": [k]}));
        let row = &v.as_array().expect("rows")[0];
        assert_eq!(
            row["created_by"],
            Value::Null,
            "the key's own creator: {row}"
        );
        assert!(row.get("created_by_id").is_none(), "{row}");
        assert!(row.get("created_by_name").is_none(), "{row}");
        assert_eq!(row["account_created_by_name"], "Admin");
        assert_eq!(row["account_created_by_id"], "1111111111111111111");
    }

    /// `created_by` null on the ACCOUNT, an object on the key: no stray
    /// unprefixed `created_by` column from the account.
    #[test]
    fn info_null_account_creator_stays_prefixed() {
        let mut acct = account();
        acct["created_by"] = Value::Null;
        let v = info_value(&json!({"service_account": acct, "keys": [key("k1")]}));
        let row = &v.as_array().expect("rows")[0];
        assert!(row.get("created_by").is_none(), "{row}");
        assert_eq!(row["account_created_by"], Value::Null);
        assert_eq!(row["created_by_name"], "Admin", "the key's own creator");
    }

    /// Mixed across keys: one key with a creator, one without. The account's
    /// creator is prefixed on every row.
    #[test]
    fn info_mixed_key_creators_prefix_the_account_creator_on_every_row() {
        let mut bare = key("k2");
        bare["created_by"] = Value::Null;
        let v = info_value(&json!({"service_account": account(), "keys": [key("k1"), bare]}));
        for row in v.as_array().expect("rows") {
            assert_eq!(row["account_created_by_name"], "Admin", "{row}");
        }
    }

    const SECRET: &str = "fio_sa_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8S9t0U1v";

    /// The flat object `create` renders (before the secret is added).
    fn create_payload() -> Map<String, Value> {
        let Value::Object(acct) = account() else {
            unreachable!()
        };
        let mut value = flatten_with_rename(&acct, "", Some("service_account_id"));
        let Value::Object(k) = key("kAbCdEfGhIjKlMnOpQrS") else {
            unreachable!()
        };
        for (name, v) in flatten_fields(&k) {
            value.insert(format!("key_{name}"), v);
        }
        value
    }

    /// The flat object `key create` renders (before the secret is added).
    fn key_create_payload() -> Map<String, Value> {
        let mut value = Map::new();
        value.insert("service_account_id".to_owned(), json!(SA_ID));
        let Value::Object(k) = key("kAbCdEfGhIjKlMnOpQrS") else {
            unreachable!()
        };
        for (name, v) in flatten_fields(&k) {
            value.insert(format!("key_{name}"), v);
        }
        value
    }

    /// The table's dynamic layout wraps long cells to the width. At realistic
    /// terminal widths the secret must still appear whole, on one line — and
    /// exactly once. Runs with colour ON (no `force_no_tty`), so the dynamic
    /// arrangement is really exercised; the width is pinned explicitly.
    #[test]
    fn table_secret_stays_on_one_line_at_fixed_widths() {
        for payload in [create_payload(), key_create_payload()] {
            for width in [80_u16, 120] {
                let text = table_with_secret_line(&payload, SECRET, false, Some(width));
                assert_eq!(text.matches(SECRET).count(), 1, "width {width}:\n{text}");
                let line = text
                    .lines()
                    .find(|l| l.contains(SECRET))
                    .expect("a line carrying the whole secret");
                assert_eq!(line, format!("secret: {SECRET}"));
                // The dynamic layout really ran: the one-row table wrapped onto
                // more than the 5 lines (border, header, rule, row, border) of
                // an unwrapped one.
                let table_part = text.split("\nsecret: ").next().unwrap_or_default();
                assert!(
                    table_part.lines().count() > 5,
                    "width {width}:\n{table_part}"
                );
            }
        }
    }

    /// Positive control for the test above: putting the secret IN the table
    /// at width 80 does split it, which is the defect the separate line avoids.
    #[test]
    fn a_secret_inside_the_table_would_be_split_at_80_columns() {
        for payload in [create_payload(), key_create_payload()] {
            let with_secret = Value::Object(with_secret_last(payload, SECRET));
            let text = table::to_text(&with_secret, false, Some(80));
            assert!(!text.contains(SECRET), "expected a wrapped secret:\n{text}");
        }
    }

    #[test]
    fn json_and_other_formats_keep_the_secret_as_the_last_field() {
        let mut payload = create_payload();
        payload.insert("secret".to_owned(), json!("server-field"));
        let value = with_secret_last(payload, SECRET);
        assert_eq!(value.keys().next_back().map(String::as_str), Some("secret"));
        assert_eq!(value["secret"], SECRET);
        assert_eq!(
            Value::Object(value)
                .to_string()
                .matches("\"secret\"")
                .count(),
            1
        );
    }
}
