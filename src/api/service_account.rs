#![allow(clippy::missing_errors_doc)]

//! Workspace service account endpoints.
//!
//! A service account is a non-human member of exactly one workspace that
//! authenticates only with API keys. These endpoints are the management
//! surface — they are called with a workspace admin's or owner's own
//! credential, never with a service account key.
//!
//! Two calls MINT a secret (`create`, which also issues the account's first
//! key, and `key_create`). The secret is returned once and can never be read
//! again, so both:
//!
//! - go out through [`ApiClient::post_once_no_redirect`] — exactly one attempt
//!   on the wire, and a 307/308 is never followed. A gateway error, timeout, or
//!   redirect after the server committed would otherwise be replayed, minting
//!   a second account or key while the first secret is lost;
//! - return a typed wrapper ([`CreatedServiceAccount`] / [`MintedKey`]) whose
//!   secret lives in a [`SecretString`] and whose `Debug` redacts it.
//!
//! Every other call returns the response as a [`Value`], like the member
//! endpoints.

use std::collections::HashMap;
use std::fmt;

use secrecy::{ExposeSecret as _, SecretString};
use serde_json::{Map, Value};

use crate::client::ApiClient;
use crate::error::CliError;

// ─── Paths (pure; unit-tested) ───────────────────────────────────────────────

/// `/workspace/{ws}/service_accounts/` — the collection path.
fn collection_path(workspace_id: &str) -> String {
    format!(
        "/workspace/{}/service_accounts/",
        urlencoding::encode(workspace_id)
    )
}

/// `/workspace/{ws}/service_accounts/create/`.
fn create_path(workspace_id: &str) -> String {
    format!("{}create/", collection_path(workspace_id))
}

/// `/workspace/{ws}/service_accounts/{sa}/` — the account itself (DELETE).
fn account_path(workspace_id: &str, service_account_id: &str) -> String {
    format!(
        "{}{}/",
        collection_path(workspace_id),
        urlencoding::encode(service_account_id)
    )
}

/// `/workspace/{ws}/service_accounts/{sa}/{action}/` for `details`, `update`,
/// `disable`, `enable`, and `keys`.
fn account_action_path(workspace_id: &str, service_account_id: &str, action: &str) -> String {
    format!(
        "{}{}/",
        account_path(workspace_id, service_account_id),
        urlencoding::encode(action)
    )
}

/// `/workspace/{ws}/service_accounts/{sa}/keys/create/`.
fn key_create_path(workspace_id: &str, service_account_id: &str) -> String {
    format!(
        "{}create/",
        account_action_path(workspace_id, service_account_id, "keys")
    )
}

/// `/workspace/{ws}/service_accounts/{sa}/keys/{key}/` — one key (DELETE).
fn key_path(workspace_id: &str, service_account_id: &str, key_id: &str) -> String {
    format!(
        "{}{}/",
        account_action_path(workspace_id, service_account_id, "keys"),
        urlencoding::encode(key_id)
    )
}

/// Reject an id that would address a different route than the one the caller
/// meant: an empty one collapses its segment (`…//…`), and `.` / `..` survive
/// percent-encoding and are resolved as dot-segments when the URL is parsed —
/// `key revoke SA ..` would otherwise become `DELETE …/service_accounts/SA/`,
/// deleting the whole account.
///
/// Every function in this module calls it for every id before any request;
/// callers may also call it up front to validate before other work.
pub fn require_id(value: &str, what: &str) -> Result<(), CliError> {
    if value.trim().is_empty() {
        return Err(CliError::Parse(format!("{what} must not be empty")));
    }
    if value == "." || value == ".." {
        return Err(CliError::Parse(format!("{what} must not be '{value}'")));
    }
    Ok(())
}

// ─── Form bodies (pure; unit-tested) ─────────────────────────────────────────

/// Parameters for [`create`].
#[derive(Debug, Clone, Copy)]
pub struct CreateParams<'a> {
    /// Display name, unique within the workspace (case-insensitive).
    pub name: &'a str,
    /// `viewer` or `editor`. Required: the server applies no default.
    pub role: &'a str,
    /// Expiry preset for the first key (`1w`, `30d`, `90d`, `1y`, `never`).
    /// Omitted when `None`; the server then uses its default.
    pub key_expires: Option<&'a str>,
    /// Optional memo for the first key.
    pub key_memo: Option<&'a str>,
}

/// Form body for [`create`]. Optional fields are omitted when unset.
fn build_create_form(params: &CreateParams<'_>) -> HashMap<String, String> {
    let mut form = HashMap::new();
    form.insert("name".to_owned(), params.name.to_owned());
    form.insert("role".to_owned(), params.role.to_owned());
    if let Some(v) = params.key_expires {
        form.insert("key_expires".to_owned(), v.to_owned());
    }
    if let Some(v) = params.key_memo {
        form.insert("key_memo".to_owned(), v.to_owned());
    }
    form
}

/// Form body for [`update`]: only the fields being changed.
fn build_update_form(name: Option<&str>, role: Option<&str>) -> HashMap<String, String> {
    let mut form = HashMap::new();
    if let Some(v) = name {
        form.insert("name".to_owned(), v.to_owned());
    }
    if let Some(v) = role {
        form.insert("role".to_owned(), v.to_owned());
    }
    form
}

/// Form body for [`key_create`]. Both fields are optional; an omitted
/// `expires` takes the server default.
fn build_key_create_form(expires: Option<&str>, memo: Option<&str>) -> HashMap<String, String> {
    let mut form = HashMap::new();
    if let Some(v) = expires {
        form.insert("expires".to_owned(), v.to_owned());
    }
    if let Some(v) = memo {
        form.insert("memo".to_owned(), v.to_owned());
    }
    form
}

// ─── Secret-bearing responses ────────────────────────────────────────────────

/// A newly minted service account key: its metadata plus the one-time secret.
///
/// The secret is held in a [`SecretString`] and is NOT part of [`Self::key`];
/// read it only through [`Self::expose_secret`], when building the output the
/// user asked for. `Debug` is implemented by hand and redacts it.
pub struct MintedKey {
    /// The key's metadata as returned by the server (`id`, `hint`, `memo`,
    /// `created`, `expires`, …), with the `secret` field removed.
    pub key: Map<String, Value>,
    /// The one-time secret.
    secret: SecretString,
}

impl MintedKey {
    /// The full key secret. It is returned only by the call that minted the
    /// key and can never be retrieved again.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        self.secret.expose_secret()
    }

    /// The key id (not the secret), when the server returned one.
    #[must_use]
    pub fn key_id(&self) -> Option<&str> {
        self.key.get("id").and_then(Value::as_str)
    }
}

impl fmt::Debug for MintedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MintedKey")
            .field("key", &self.key)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

/// The result of [`create`]: the new account and its first key.
pub struct CreatedServiceAccount {
    /// The service account object as returned by the server; `None` when the
    /// response lacked it (the key and its secret are still returned).
    pub service_account: Option<Map<String, Value>>,
    /// The account's first key, including its one-time secret.
    pub key: MintedKey,
}

impl fmt::Debug for CreatedServiceAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `MintedKey`'s own `Debug` redacts the secret; spelled out by hand so
        // a future field holding raw response JSON cannot slip in unredacted.
        f.debug_struct("CreatedServiceAccount")
            .field("service_account", &self.service_account)
            .field("key", &self.key)
            .finish()
    }
}

/// Turn a response's `key` object into a [`MintedKey`], moving the secret out
/// of the metadata map.
///
/// Error messages are static: they never embed the response, which carries
/// the secret.
fn parse_minted_key(key: Option<Value>) -> Result<MintedKey, CliError> {
    let Some(Value::Object(mut key)) = key else {
        return Err(CliError::Parse(
            "the response did not include the new key".to_owned(),
        ));
    };
    match key.shift_remove("secret") {
        Some(Value::String(secret)) if !secret.is_empty() => Ok(MintedKey {
            key,
            secret: SecretString::from(secret),
        }),
        _ => Err(CliError::Parse(
            "the response did not include the new key's secret".to_owned(),
        )),
    }
}

/// Parse a `create` response: `{ service_account: {...}, key: {..., secret} }`.
fn parse_create_response(body: Value) -> Result<CreatedServiceAccount, CliError> {
    let Value::Object(mut body) = body else {
        return Err(CliError::Parse(
            "unexpected service account create response".to_owned(),
        ));
    };
    let key = parse_minted_key(body.shift_remove("key"))?;
    // A missing account object must not cost the user the secret that DID
    // arrive: keep the key and let the caller say what is missing.
    let service_account = match body.shift_remove("service_account") {
        Some(Value::Object(service_account)) => Some(service_account),
        _ => None,
    };
    Ok(CreatedServiceAccount {
        service_account,
        key,
    })
}

/// Parse a `key create` response: `{ key: {..., secret} }`.
fn parse_key_create_response(body: Value) -> Result<MintedKey, CliError> {
    let Value::Object(mut body) = body else {
        return Err(CliError::Parse(
            "unexpected service account key create response".to_owned(),
        ));
    };
    parse_minted_key(body.shift_remove("key"))
}

// ─── Endpoints ───────────────────────────────────────────────────────────────

/// List a workspace's service accounts and the plan limit.
///
/// `GET /workspace/{ws}/service_accounts/` →
/// `{ service_accounts: [...], limit: { max, used } }`
pub async fn list(client: &ApiClient, workspace_id: &str) -> Result<Value, CliError> {
    require_id(workspace_id, "workspace ID")?;
    client.get(&collection_path(workspace_id)).await
}

/// Create a service account together with its first key.
///
/// `POST /workspace/{ws}/service_accounts/create/` →
/// `{ service_account, key }` where `key` carries the one-time `secret`.
///
/// Sent exactly once (see the module docs): a transport failure, a 5xx, or an
/// unreadable success body leaves it UNKNOWN whether the account exists, and
/// the caller must tell the user how to check rather than retry.
pub async fn create(
    client: &ApiClient,
    workspace_id: &str,
    params: &CreateParams<'_>,
) -> Result<CreatedServiceAccount, CliError> {
    require_id(workspace_id, "workspace ID")?;
    let body: Value = client
        .post_once_no_redirect(&create_path(workspace_id), &build_create_form(params))
        .await?;
    parse_create_response(body)
}

/// Get a service account and its keys (never their secrets).
///
/// `GET /workspace/{ws}/service_accounts/{sa}/details/` →
/// `{ service_account, keys: [...] }`
pub async fn details(
    client: &ApiClient,
    workspace_id: &str,
    service_account_id: &str,
) -> Result<Value, CliError> {
    require_id(workspace_id, "workspace ID")?;
    require_id(service_account_id, "service account ID")?;
    client
        .get(&account_action_path(
            workspace_id,
            service_account_id,
            "details",
        ))
        .await
}

/// Rename a service account and/or change its role. A role change rewrites
/// the scope of every key the account holds.
///
/// `POST /workspace/{ws}/service_accounts/{sa}/update/` → `{ service_account }`
pub async fn update(
    client: &ApiClient,
    workspace_id: &str,
    service_account_id: &str,
    name: Option<&str>,
    role: Option<&str>,
) -> Result<Value, CliError> {
    require_id(workspace_id, "workspace ID")?;
    require_id(service_account_id, "service account ID")?;
    client
        .post(
            &account_action_path(workspace_id, service_account_id, "update"),
            &build_update_form(name, role),
        )
        .await
}

/// Disable a service account: every key stops working at once. Reversible.
///
/// `POST /workspace/{ws}/service_accounts/{sa}/disable/` → `{ service_account }`
pub async fn disable(
    client: &ApiClient,
    workspace_id: &str,
    service_account_id: &str,
) -> Result<Value, CliError> {
    require_id(workspace_id, "workspace ID")?;
    require_id(service_account_id, "service account ID")?;
    client
        .post(
            &account_action_path(workspace_id, service_account_id, "disable"),
            &HashMap::new(),
        )
        .await
}

/// Re-enable a disabled service account.
///
/// `POST /workspace/{ws}/service_accounts/{sa}/enable/` → `{ service_account }`
pub async fn enable(
    client: &ApiClient,
    workspace_id: &str,
    service_account_id: &str,
) -> Result<Value, CliError> {
    require_id(workspace_id, "workspace ID")?;
    require_id(service_account_id, "service account ID")?;
    client
        .post(
            &account_action_path(workspace_id, service_account_id, "enable"),
            &HashMap::new(),
        )
        .await
}

/// Delete a service account: its keys are deleted and the account is closed.
///
/// `DELETE /workspace/{ws}/service_accounts/{sa}/` → `{}`
pub async fn delete(
    client: &ApiClient,
    workspace_id: &str,
    service_account_id: &str,
) -> Result<Value, CliError> {
    require_id(workspace_id, "workspace ID")?;
    require_id(service_account_id, "service account ID")?;
    client
        .delete(&account_path(workspace_id, service_account_id))
        .await
}

/// List a service account's keys (never their secrets).
///
/// `GET /workspace/{ws}/service_accounts/{sa}/keys/` → `{ keys: [...] }`
pub async fn keys_list(
    client: &ApiClient,
    workspace_id: &str,
    service_account_id: &str,
) -> Result<Value, CliError> {
    require_id(workspace_id, "workspace ID")?;
    require_id(service_account_id, "service account ID")?;
    client
        .get(&account_action_path(
            workspace_id,
            service_account_id,
            "keys",
        ))
        .await
}

/// Mint a new key for a service account.
///
/// `POST /workspace/{ws}/service_accounts/{sa}/keys/create/` → `{ key }` where
/// `key` carries the one-time `secret`. Sent exactly once, like [`create`].
pub async fn key_create(
    client: &ApiClient,
    workspace_id: &str,
    service_account_id: &str,
    expires: Option<&str>,
    memo: Option<&str>,
) -> Result<MintedKey, CliError> {
    require_id(workspace_id, "workspace ID")?;
    require_id(service_account_id, "service account ID")?;
    let body: Value = client
        .post_once_no_redirect(
            &key_create_path(workspace_id, service_account_id),
            &build_key_create_form(expires, memo),
        )
        .await?;
    parse_key_create_response(body)
}

/// Revoke (delete) one key. Immediate.
///
/// `DELETE /workspace/{ws}/service_accounts/{sa}/keys/{key}/` → `{}`
pub async fn key_revoke(
    client: &ApiClient,
    workspace_id: &str,
    service_account_id: &str,
    key_id: &str,
) -> Result<Value, CliError> {
    require_id(workspace_id, "workspace ID")?;
    require_id(service_account_id, "service account ID")?;
    require_id(key_id, "key ID")?;
    client
        .delete(&key_path(workspace_id, service_account_id, key_id))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const WS: &str = "1234567890123456789";
    const SA: &str = "9876543210987654321";
    const SECRET: &str = "fio_sa_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789abcde";

    #[test]
    fn paths_match_the_endpoint_table_exactly() {
        assert_eq!(
            collection_path(WS),
            format!("/workspace/{WS}/service_accounts/")
        );
        assert_eq!(
            create_path(WS),
            format!("/workspace/{WS}/service_accounts/create/")
        );
        assert_eq!(
            account_path(WS, SA),
            format!("/workspace/{WS}/service_accounts/{SA}/")
        );
        for action in ["details", "update", "disable", "enable", "keys"] {
            assert_eq!(
                account_action_path(WS, SA, action),
                format!("/workspace/{WS}/service_accounts/{SA}/{action}/")
            );
        }
        assert_eq!(
            key_create_path(WS, SA),
            format!("/workspace/{WS}/service_accounts/{SA}/keys/create/")
        );
        assert_eq!(
            key_path(WS, SA, "k1"),
            format!("/workspace/{WS}/service_accounts/{SA}/keys/k1/")
        );
    }

    /// Every id segment is percent-encoded so an id cannot break out of its
    /// slot and address another route.
    #[test]
    fn path_segments_are_percent_encoded() {
        let p = key_path("a/b", "c d", "e?f");
        assert_eq!(p, "/workspace/a%2Fb/service_accounts/c%20d/keys/e%3Ff/");
        assert!(account_path("a/b", "../x").contains("..%2Fx"));
        assert!(create_path("a/b").starts_with("/workspace/a%2Fb/"));
        assert!(account_action_path("1", "x/y", "details").contains("x%2Fy/details/"));
    }

    #[test]
    fn empty_ids_are_refused() {
        assert!(require_id("", "workspace ID").is_err());
        assert!(require_id("  ", "workspace ID").is_err());
        assert!(require_id("1", "workspace ID").is_ok());
    }

    /// `.` and `..` survive percent-encoding and are dot-segments, so they are
    /// refused in every id position. `...` or `a..b` are ordinary segments.
    #[test]
    fn dot_segment_ids_are_refused() {
        for id in [".", ".."] {
            for what in ["workspace ID", "service account ID", "key ID"] {
                let err = require_id(id, what).unwrap_err();
                assert!(err.to_string().contains(what), "{err}");
            }
        }
        assert!(require_id("...", "key ID").is_ok());
        assert!(require_id("a..b", "key ID").is_ok());
    }

    /// Parse the final URL the way the client does (base + path) and show the
    /// path survives intact for real ids, while a raw `..` WOULD collapse a key
    /// DELETE onto the account DELETE — which is why `require_id` refuses it.
    #[test]
    fn final_urls_keep_every_segment() {
        let base = "https://api.example.test/current";
        let url = |path: String| {
            reqwest::Url::parse(&format!("{base}{path}"))
                .expect("valid url")
                .path()
                .to_owned()
        };
        assert_eq!(
            url(key_path(WS, SA, "k1")),
            format!("/current/workspace/{WS}/service_accounts/{SA}/keys/k1/")
        );
        assert_eq!(
            url(account_action_path(WS, SA, "details")),
            format!("/current/workspace/{WS}/service_accounts/{SA}/details/")
        );
        // Positive control for the hazard itself.
        assert_eq!(
            url(key_path(WS, SA, "..")),
            format!("/current/workspace/{WS}/service_accounts/{SA}/")
        );
    }

    #[test]
    fn create_form_carries_required_fields_and_omits_unset_options() {
        let form = build_create_form(&CreateParams {
            name: "ci-bot",
            role: "editor",
            key_expires: None,
            key_memo: None,
        });
        assert_eq!(form.len(), 2, "{form:?}");
        assert_eq!(form.get("name").map(String::as_str), Some("ci-bot"));
        assert_eq!(form.get("role").map(String::as_str), Some("editor"));
        assert!(!form.contains_key("key_expires"));
        assert!(!form.contains_key("key_memo"));
    }

    #[test]
    fn create_form_carries_key_options_under_their_wire_names() {
        let form = build_create_form(&CreateParams {
            name: "ci-bot",
            role: "viewer",
            key_expires: Some("1y"),
            key_memo: Some("deploys"),
        });
        assert_eq!(form.len(), 4, "{form:?}");
        assert_eq!(form.get("key_expires").map(String::as_str), Some("1y"));
        assert_eq!(form.get("key_memo").map(String::as_str), Some("deploys"));
        // The key-create spellings belong to the other endpoint.
        assert!(!form.contains_key("expires") && !form.contains_key("memo"));
    }

    #[test]
    fn update_form_sends_only_the_changed_fields() {
        assert!(build_update_form(None, None).is_empty());
        let name_only = build_update_form(Some("renamed"), None);
        assert_eq!(name_only.len(), 1);
        assert_eq!(name_only.get("name").map(String::as_str), Some("renamed"));
        let role_only = build_update_form(None, Some("viewer"));
        assert_eq!(role_only.len(), 1);
        assert_eq!(role_only.get("role").map(String::as_str), Some("viewer"));
        let both = build_update_form(Some("n"), Some("editor"));
        assert_eq!(both.len(), 2);
    }

    #[test]
    fn key_create_form_omits_unset_options() {
        assert!(build_key_create_form(None, None).is_empty());
        let form = build_key_create_form(Some("never"), Some("rotation"));
        assert_eq!(form.len(), 2);
        assert_eq!(form.get("expires").map(String::as_str), Some("never"));
        assert_eq!(form.get("memo").map(String::as_str), Some("rotation"));
        assert!(!form.contains_key("key_expires"));
    }

    fn key_json() -> Value {
        json!({
            "id": "k1",
            "hint": "fio_sa_…cde",
            "memo": "",
            "created": "2026-10-16 12:00:00 UTC",
            "expires": null,
            "expired": false,
            "last_used": null,
            "secret": SECRET,
        })
    }

    #[test]
    fn create_response_moves_the_secret_out_of_the_metadata() {
        let created = parse_create_response(json!({
            "result": "yes",
            "service_account": {"id": SA, "name": "ci-bot"},
            "key": key_json(),
        }))
        .expect("parse");
        assert_eq!(created.key.expose_secret(), SECRET);
        assert_eq!(created.key.key_id(), Some("k1"));
        assert!(!created.key.key.contains_key("secret"));
        assert_eq!(
            created.service_account.as_ref().map(|a| &a["name"]),
            Some(&json!("ci-bot"))
        );
    }

    #[test]
    fn key_create_response_moves_the_secret_out_of_the_metadata() {
        let minted = parse_key_create_response(json!({ "key": key_json() })).expect("parse");
        assert_eq!(minted.expose_secret(), SECRET);
        assert!(!minted.key.contains_key("secret"));
    }

    /// A malformed success must fail with a STATIC message — never one that
    /// embeds the response, which may still carry the secret.
    #[test]
    fn malformed_responses_fail_without_echoing_the_body() {
        let no_secret = parse_key_create_response(json!({"key": {"id": "k1"}})).unwrap_err();
        assert!(no_secret.to_string().contains("secret"), "{no_secret}");

        let not_object = parse_create_response(json!([SECRET])).unwrap_err();
        assert!(!not_object.to_string().contains(SECRET), "{not_object}");

        let empty_secret =
            parse_key_create_response(json!({"key": {"id": "k1", "secret": ""}})).unwrap_err();
        assert!(empty_secret.to_string().contains("secret"));
    }

    /// A create response whose account object is missing (or not an object)
    /// still yields the key and its secret — the secret is never discarded.
    #[test]
    fn create_without_an_account_object_keeps_the_secret() {
        for body in [
            json!({"key": key_json()}),
            json!({"service_account": "oops", "key": key_json()}),
            json!({"service_account": null, "key": key_json()}),
        ] {
            let created = parse_create_response(body).expect("the key still parses");
            assert!(created.service_account.is_none());
            assert_eq!(created.key.expose_secret(), SECRET);
            assert_eq!(created.key.key_id(), Some("k1"));
        }
    }

    #[test]
    fn debug_redacts_the_secret() {
        let created = parse_create_response(json!({
            "service_account": {"id": SA, "name": "ci-bot"},
            "key": key_json(),
        }))
        .expect("parse");
        let debug = format!("{created:?}");
        assert!(!debug.contains(SECRET), "{debug}");
        assert!(debug.contains("[REDACTED]"), "{debug}");
        assert!(debug.contains("ci-bot"), "metadata stays visible: {debug}");

        let alt = format!("{:#?}", created.key);
        assert!(!alt.contains(SECRET), "{alt}");
        assert!(alt.contains("[REDACTED]"), "{alt}");
    }
}
