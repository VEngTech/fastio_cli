/// Comment command implementations for `fastio comment *`.
///
/// Handles listing, creating, replying to, and deleting comments
/// on workspace and share files.
use anyhow::{Context, Result};
use colored::Colorize;
use serde_json::{Value, json};

use super::CommandContext;
use crate::cli::CommentFilterArgs;
use fastio_cli::api;
use fastio_cli::output::OutputFormat;

/// A `--quote` anchor request as given on the command line.
#[derive(Debug, Clone)]
pub struct QuoteArgs {
    /// The quoted passage.
    pub exact: String,
    /// Explicit text before it.
    pub prefix: Option<String>,
    /// Explicit text after it.
    pub suffix: Option<String>,
    /// 1-based choice among repeated matches.
    pub occurrence: Option<usize>,
}

impl QuoteArgs {
    /// The API-layer request these flags describe.
    #[must_use]
    pub fn as_request(&self) -> api::comment::QuoteRequest<'_> {
        api::comment::QuoteRequest {
            exact: &self.exact,
            prefix: self.prefix.as_deref(),
            suffix: self.suffix.as_deref(),
            occurrence: self.occurrence,
        }
    }
}

/// The `comment assign` target flags (clap guarantees exactly one of
/// `assign_self`, `ripley`, `user_id`).
#[derive(Debug, Clone, Default)]
pub struct AssignTarget {
    /// `--self`.
    pub assign_self: bool,
    /// `--ripley`.
    pub ripley: bool,
    /// `--user-id`.
    pub user_id: Option<String>,
    /// `--agent-name` (with `--user-id`).
    pub agent_name: Option<String>,
    /// `--credential-type` (with `--agent-name`).
    pub credential_type: Option<String>,
    /// `--credential-id` (with `--agent-name`).
    pub credential_id: Option<String>,
}

impl AssignTarget {
    /// The API-layer assignee these flags describe.
    fn to_assignee(&self) -> Result<api::comment::Assignee<'_>> {
        match (self.assign_self, self.ripley, self.user_id.as_deref()) {
            (true, false, None) => Ok(api::comment::Assignee::Caller),
            (false, true, None) => Ok(api::comment::Assignee::Ripley),
            (false, false, Some(user_id)) => {
                anyhow::ensure!(!user_id.trim().is_empty(), "--user-id must not be empty");
                let agent = match (self.agent_name.as_deref(), self.credential_type.as_deref()) {
                    (Some(agent_name), Some(credential_type)) => {
                        Some(api::comment::AgentQualifier {
                            agent_name,
                            credential_type,
                            credential_id: self.credential_id.as_deref(),
                        })
                    }
                    (None, None) => None,
                    _ => anyhow::bail!("--agent-name and --credential-type go together"),
                };
                Ok(api::comment::Assignee::User { user_id, agent })
            }
            _ => anyhow::bail!("pass exactly one of --self, --ripley or --user-id"),
        }
    }
}

/// Comment subcommand variants.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum CommentCommand {
    /// List comments on a file.
    List {
        /// Entity type: workspace or share.
        entity_type: String,
        /// Entity ID (workspace or share ID).
        entity_id: String,
        /// Storage node ID.
        node_id: String,
        /// Sort order.
        sort: Option<String>,
        /// Max results.
        limit: Option<u32>,
        /// Offset for pagination.
        offset: Option<u32>,
        /// Thread filters.
        filters: CommentFilterArgs,
    },
    /// Add a comment to a file.
    Create {
        /// Entity type: workspace or share.
        entity_type: String,
        /// Entity ID.
        entity_id: String,
        /// Storage node ID.
        node_id: String,
        /// Comment text.
        text: String,
        /// Anchoring reference as a JSON object string (or `@file.json`).
        reference: Option<String>,
        /// Quote anchor (mutually exclusive with `reference`).
        quote: Option<QuoteArgs>,
        /// Arbitrary metadata as a JSON object string (or `@file.json`).
        properties: Option<String>,
        /// Inline-attach a single object (object ID).
        target_id: Option<String>,
        /// Inline-attach multiple objects (object IDs, ≤25).
        target_ids: Vec<String>,
    },
    /// Reply to an existing comment.
    Reply {
        /// Entity type: workspace or share.
        entity_type: String,
        /// Entity ID.
        entity_id: String,
        /// Storage node ID.
        node_id: String,
        /// Parent comment ID to reply to.
        comment_id: String,
        /// Reply text.
        text: String,
        /// Also reopen the thread.
        reopen: bool,
    },
    /// Resolve a comment thread.
    Resolve {
        /// Root comment ID.
        comment_id: String,
        /// Optional state-version precondition.
        if_state_version: Option<u64>,
    },
    /// Reopen a comment thread.
    Reopen {
        /// Root comment ID.
        comment_id: String,
        /// Optional state-version precondition.
        if_state_version: Option<u64>,
    },
    /// Assign a comment thread.
    Assign {
        /// Root comment ID.
        comment_id: String,
        /// Who to assign.
        target: AssignTarget,
    },
    /// Clear a comment thread's assignee.
    Unassign {
        /// Root comment ID.
        comment_id: String,
    },
    /// Edit a comment's text (author-only; by comment ID).
    Edit {
        /// Comment ID.
        comment_id: String,
        /// New comment text.
        text: String,
    },
    /// Delete a comment.
    Delete {
        /// Comment ID.
        comment_id: String,
    },
    /// List all comments across an entity (workspace or share).
    ListAll {
        /// Entity type: workspace or share.
        entity_type: String,
        /// Entity ID (workspace or share ID).
        entity_id: String,
        /// Sort order.
        sort: Option<String>,
        /// Max results.
        limit: Option<u32>,
        /// Offset for pagination.
        offset: Option<u32>,
        /// Thread filters.
        filters: CommentFilterArgs,
    },
    /// Get comment details.
    Info {
        /// Comment ID.
        comment_id: String,
    },
    /// Add an emoji reaction to a comment.
    React {
        /// Comment ID.
        comment_id: String,
        /// Emoji to react with.
        emoji: String,
    },
    /// Remove your emoji reaction from a comment.
    Unreact {
        /// Comment ID.
        comment_id: String,
    },
    /// Bulk soft-delete up to 100 comments by ID.
    BulkDelete {
        /// Comment IDs to delete.
        comment_ids: Vec<String>,
    },
    /// List the objects attached to a comment.
    Attachments {
        /// Comment ID.
        comment_id: String,
    },
    /// Attach one or more objects to a comment.
    Attach {
        /// Comment ID.
        comment_id: String,
        /// Attach a single object (object ID).
        target_id: Option<String>,
        /// Attach multiple objects (object IDs, ≤25).
        target_ids: Vec<String>,
    },
    /// Detach a single object from a comment.
    Detach {
        /// Comment ID.
        comment_id: String,
        /// Object ID to detach.
        target_id: String,
    },
}

/// Entity types valid on the **node-scoped** comment routes
/// (`/comments/{entity_type}/{parent_id}/{node_id}/`).
///
/// `fileshare` is the platform's third entity type, used by File Share
/// recipients to read and post comments on a file inside a File Share. Omitting
/// it here makes the CLI reject client-side a call the platform accepts.
pub(crate) const VALID_ENTITY_TYPES: &[&str] = &["workspace", "share", "fileshare"];

/// Entity types valid on the **container-scoped** route
/// (`/comments/{entity_type}/{parent_id}/`, no node segment).
///
/// Deliberately NOT the same list. Per the published API docs, for `fileshare`
/// the "`{node_id}` segment is **required** (a File Share comment always targets
/// a specific node, never the whole container)". Adding `fileshare` to one
/// shared allow-list would trade a false rejection for a request the platform
/// does not serve.
pub(crate) const CONTAINER_ENTITY_TYPES: &[&str] = &["workspace", "share"];

/// Validate entity type for a node-scoped comment route.
fn validate_entity_type(entity_type: &str) -> Result<()> {
    anyhow::ensure!(
        VALID_ENTITY_TYPES.contains(&entity_type),
        "invalid entity type '{entity_type}'. Must be one of: workspace, share, fileshare"
    );
    Ok(())
}

/// Validate entity type for the container-scoped listing, which has no node
/// segment and therefore cannot serve a File Share.
fn validate_container_entity_type(entity_type: &str) -> Result<()> {
    if entity_type == "fileshare" {
        anyhow::bail!(
            "entity type 'fileshare' requires a node: a File Share comment always \
             targets a specific file, never the whole container. Use `fastio comment \
             list --entity-type fileshare --entity-id <id> --node-id <node>` instead."
        );
    }
    anyhow::ensure!(
        CONTAINER_ENTITY_TYPES.contains(&entity_type),
        "invalid entity type '{entity_type}'. Must be one of: workspace, share"
    );
    Ok(())
}

/// Validate entity type and IDs common to several comment subcommands.
fn validate_entity_args(entity_type: &str, entity_id: &str, node_id: &str) -> Result<()> {
    validate_entity_type(entity_type)?;
    anyhow::ensure!(!entity_id.trim().is_empty(), "entity ID must not be empty");
    anyhow::ensure!(!node_id.trim().is_empty(), "node ID must not be empty");
    Ok(())
}

/// Execute a comment subcommand.
#[allow(clippy::too_many_lines)]
pub async fn execute(command: &CommentCommand, ctx: &CommandContext<'_>) -> Result<()> {
    match command {
        CommentCommand::List {
            entity_type,
            entity_id,
            node_id,
            sort,
            limit,
            offset,
            filters,
        } => {
            validate_entity_args(entity_type, entity_id, node_id)?;
            let client = ctx.build_client()?;
            let value = api::comment::list_comments(
                &client,
                &api::comment::ListCommentsParams {
                    entity_type,
                    entity_id,
                    node_id,
                    sort: sort.as_deref(),
                    limit: *limit,
                    offset: *offset,
                    filters: filters.to_api(),
                },
            )
            .await
            .context("failed to list comments")?;
            render_list(ctx, value)
        }
        CommentCommand::Create {
            entity_type,
            entity_id,
            node_id,
            text,
            reference,
            quote,
            properties,
            target_id,
            target_ids,
        } => {
            validate_entity_args(entity_type, entity_id, node_id)?;
            create(
                ctx,
                &CreateArgs {
                    entity_type,
                    entity_id,
                    node_id,
                    text,
                    reference: reference.as_deref(),
                    quote: quote.as_ref(),
                    properties: properties.as_deref(),
                    target_id: target_id.as_deref(),
                    target_ids,
                },
            )
            .await
        }
        CommentCommand::Reply {
            entity_type,
            entity_id,
            node_id,
            comment_id,
            text,
            reopen,
        } => {
            validate_entity_args(entity_type, entity_id, node_id)?;
            reply(
                ctx,
                &api::comment::AddCommentParams {
                    entity_type,
                    entity_id,
                    node_id,
                    body: text,
                    parent_id: Some(comment_id),
                    reference: None,
                    properties: None,
                    target_id: None,
                    target_ids: None,
                    reopen: *reopen,
                },
            )
            .await
        }
        CommentCommand::Resolve {
            comment_id,
            if_state_version,
        } => {
            let client = ctx.build_client()?;
            let value = api::comment::resolve_comment(&client, comment_id, *if_state_version)
                .await
                .context("failed to resolve comment thread")?;
            ctx.output.render(&value)?;
            Ok(())
        }
        CommentCommand::Reopen {
            comment_id,
            if_state_version,
        } => {
            let client = ctx.build_client()?;
            let value = api::comment::reopen_comment(&client, comment_id, *if_state_version)
                .await
                .context("failed to reopen comment thread")?;
            ctx.output.render(&value)?;
            Ok(())
        }
        CommentCommand::Assign { comment_id, target } => {
            let assignee = target.to_assignee()?;
            let client = ctx.build_client()?;
            let value = api::comment::assign_comment(&client, comment_id, &assignee)
                .await
                .context("failed to assign comment thread")?;
            ctx.output.render(&value)?;
            Ok(())
        }
        CommentCommand::Unassign { comment_id } => {
            let client = ctx.build_client()?;
            let value = api::comment::unassign_comment(&client, comment_id)
                .await
                .context("failed to unassign comment thread")?;
            ctx.output.render(&value)?;
            Ok(())
        }
        CommentCommand::Edit { comment_id, text } => edit(ctx, comment_id, text).await,
        CommentCommand::Delete { comment_id } => delete(ctx, comment_id).await,
        CommentCommand::ListAll {
            entity_type,
            entity_id,
            sort,
            limit,
            offset,
            filters,
        } => {
            validate_container_entity_type(entity_type)?;
            anyhow::ensure!(!entity_id.trim().is_empty(), "entity ID must not be empty");
            let client = ctx.build_client()?;
            let value = api::comment::list_all_comments(
                &client,
                entity_type,
                entity_id,
                sort.as_deref(),
                *limit,
                *offset,
                &filters.to_api(),
            )
            .await
            .context("failed to list all comments")?;
            render_list(ctx, value)
        }
        CommentCommand::Info { comment_id } => info(ctx, comment_id).await,
        CommentCommand::React { comment_id, emoji } => react(ctx, comment_id, emoji).await,
        CommentCommand::Unreact { comment_id } => unreact(ctx, comment_id).await,
        CommentCommand::BulkDelete { comment_ids } => bulk_delete(ctx, comment_ids).await,
        CommentCommand::Attachments { comment_id } => attachments(ctx, comment_id).await,
        CommentCommand::Attach {
            comment_id,
            target_id,
            target_ids,
        } => attach(ctx, comment_id, target_id.as_deref(), target_ids).await,
        CommentCommand::Detach {
            comment_id,
            target_id,
        } => detach(ctx, comment_id, target_id).await,
    }
}

/// Arguments for [`create`].
struct CreateArgs<'a> {
    entity_type: &'a str,
    entity_id: &'a str,
    node_id: &'a str,
    text: &'a str,
    reference: Option<&'a str>,
    quote: Option<&'a QuoteArgs>,
    properties: Option<&'a str>,
    target_id: Option<&'a str>,
    target_ids: &'a [String],
}

/// Add a comment.
///
/// Supports the optional create extensions: an anchoring `reference` (or a
/// `--quote` anchor built from the note's text), arbitrary `properties`, and
/// inline attachment(s) (`target_id` single or `target_ids` batch, at most
/// 25). `--target-id` / `--target-ids` are mutually exclusive (enforced by
/// clap), so an empty `target_ids` slice means "single or none".
async fn create(ctx: &CommandContext<'_>, args: &CreateArgs<'_>) -> Result<()> {
    let properties = super::parse_json_object_arg(args.properties, "properties")?;
    let target_ids = clean_target_ids(args.target_ids);
    anyhow::ensure!(
        target_ids.len() <= 25,
        "a comment accepts at most 25 attachments (got {})",
        target_ids.len()
    );
    let client = ctx.build_client()?;
    let reference = match args.quote {
        Some(quote) => Some(
            quote_reference(
                &client,
                args.entity_type,
                args.entity_id,
                args.node_id,
                quote,
            )
            .await?,
        ),
        None => super::parse_json_object_arg(args.reference, "reference")?,
    };
    let value = api::comment::add_comment(
        &client,
        &api::comment::AddCommentParams {
            entity_type: args.entity_type,
            entity_id: args.entity_id,
            node_id: args.node_id,
            body: args.text,
            parent_id: None,
            reference: reference.as_ref(),
            properties: properties.as_ref(),
            target_id: args.target_id,
            target_ids: (!target_ids.is_empty()).then_some(target_ids.as_slice()),
            reopen: false,
        },
    )
    .await
    .context("failed to create comment")?;
    ctx.output.render(&value)?;
    Ok(())
}

/// Build a quote anchor's `reference` from the note's text, warning on
/// stderr when the quote is not in the stored text (it is still sent).
pub(crate) async fn quote_reference(
    client: &fastio_cli::client::ApiClient,
    entity_type: &str,
    entity_id: &str,
    node_id: &str,
    quote: &QuoteArgs,
) -> Result<Value> {
    let (reference, matched) = api::comment::build_quote_reference(
        client,
        entity_type,
        entity_id,
        node_id,
        &quote.as_request(),
    )
    .await
    .context("failed to anchor the quote")?;
    if matched == api::comment::QuoteMatch::NotFound {
        eprintln!(
            "{} {}",
            "warning:".yellow().bold(),
            api::comment::QUOTE_NOT_FOUND_WARNING
        );
    }
    Ok(reference)
}

/// Reply to an existing comment (optionally reopening its thread).
pub(crate) async fn reply(
    ctx: &CommandContext<'_>,
    params: &api::comment::AddCommentParams<'_>,
) -> Result<()> {
    let client = ctx.build_client()?;
    let value = api::comment::add_comment(&client, params)
        .await
        .context("failed to reply to comment")?;
    ctx.output.render(&value)?;
    Ok(())
}

/// Render a comment listing.
///
/// For table and CSV, the response's `counts` and `limits` summaries are
/// moved to one stderr line, so stdout stays one clean table or CSV document
/// and the summary is not lost. JSON and markdown render the whole response.
pub(crate) fn render_list(ctx: &CommandContext<'_>, mut value: Value) -> Result<()> {
    if matches!(ctx.output.format, OutputFormat::Table | OutputFormat::Csv)
        && let Some(summary) = take_list_summary(&mut value)
        && !ctx.output.quiet
    {
        eprintln!("{summary}");
    }
    ctx.output.render(&value)?;
    Ok(())
}

/// Remove `counts` and `limits` from a listing and describe them in one
/// line. `None` when neither carries anything to say.
fn take_list_summary(value: &mut Value) -> Option<String> {
    let obj = value.as_object_mut()?;
    let counts = obj.remove("counts");
    let limits = obj.remove("limits");
    let mut parts = Vec::new();
    if let Some(Value::Object(counts)) = &counts {
        let pairs: Vec<String> = counts
            .iter()
            .filter_map(|(k, v)| v.as_u64().map(|n| format!("{k}={n}")))
            .collect();
        if !pairs.is_empty() {
            parts.push(format!("counts: {}", pairs.join(" ")));
        }
    }
    if let Some(Value::Object(limits)) = &limits
        && let Some(used) = limits.get("root_threads_used").and_then(Value::as_u64)
    {
        let max = limits
            .get("root_threads_max")
            .and_then(Value::as_u64)
            .map_or_else(|| "unlimited".to_owned(), |m| m.to_string());
        parts.push(format!("threads: {used} of {max}"));
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// Edit a comment's text.
async fn edit(ctx: &CommandContext<'_>, comment_id: &str, text: &str) -> Result<()> {
    let client = ctx.build_client()?;
    let value = api::comment::update_comment(&client, comment_id, text)
        .await
        .context("failed to edit comment")?;
    ctx.output.render(&value)?;
    Ok(())
}

/// Delete a comment.
async fn delete(ctx: &CommandContext<'_>, comment_id: &str) -> Result<()> {
    let client = ctx.build_client()?;
    api::comment::delete_comment(&client, comment_id)
        .await
        .context("failed to delete comment")?;
    let value = json!({
        "status": "deleted",
        "comment_id": comment_id,
    });
    ctx.output.render(&value)?;
    Ok(())
}

/// Get comment details.
async fn info(ctx: &CommandContext<'_>, comment_id: &str) -> Result<()> {
    let client = ctx.build_client()?;
    let value = api::comment::get_comment_details(&client, comment_id)
        .await
        .context("failed to get comment details")?;
    ctx.output.render(&value)?;
    Ok(())
}

/// Add an emoji reaction to a comment.
async fn react(ctx: &CommandContext<'_>, comment_id: &str, emoji: &str) -> Result<()> {
    let client = ctx.build_client()?;
    let value = api::comment::add_reaction(&client, comment_id, emoji)
        .await
        .context("failed to add reaction")?;
    ctx.output.render(&value)?;
    Ok(())
}

/// Remove your emoji reaction from a comment.
async fn unreact(ctx: &CommandContext<'_>, comment_id: &str) -> Result<()> {
    let client = ctx.build_client()?;
    let value = api::comment::remove_reaction(&client, comment_id)
        .await
        .context("failed to remove reaction")?;
    ctx.output.render(&value)?;
    Ok(())
}

/// Bulk soft-delete up to 100 comments.
async fn bulk_delete(ctx: &CommandContext<'_>, comment_ids: &[String]) -> Result<()> {
    let ids: Vec<String> = comment_ids
        .iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    anyhow::ensure!(!ids.is_empty(), "at least one comment ID is required");
    anyhow::ensure!(
        ids.len() <= 100,
        "bulk-delete accepts at most 100 comment ids (got {})",
        ids.len()
    );
    let client = ctx.build_client()?;
    let value = api::comment::bulk_delete_comments(&client, &ids)
        .await
        .context("failed to bulk-delete comments")?;
    ctx.output.render(&value)?;
    Ok(())
}

/// List the objects attached to a comment.
async fn attachments(ctx: &CommandContext<'_>, comment_id: &str) -> Result<()> {
    let client = ctx.build_client()?;
    let value = api::comment::list_comment_attachments(&client, comment_id)
        .await
        .context("failed to list comment attachments")?;
    ctx.output.render(&value)?;
    Ok(())
}

/// Attach one or more objects to a comment.
///
/// `--target-id` / `--target-ids` are mutually exclusive (enforced by clap);
/// exactly one must be supplied (guarded here).
async fn attach(
    ctx: &CommandContext<'_>,
    comment_id: &str,
    target_id: Option<&str>,
    target_ids: &[String],
) -> Result<()> {
    let ids = clean_target_ids(target_ids);
    let targets = match (target_id, ids.is_empty()) {
        (Some(id), true) => api::comment::CommentAttachTargets::Single(id),
        (None, false) => api::comment::CommentAttachTargets::Multiple(ids.as_slice()),
        (Some(_), false) => {
            // Defensive: clap's `conflicts_with` already rejects this pairing.
            anyhow::bail!("--target-id and --target-ids are mutually exclusive");
        }
        (None, true) => {
            anyhow::bail!("one of --target-id or --target-ids is required");
        }
    };
    anyhow::ensure!(
        ids.len() <= 25,
        "a comment accepts at most 25 attachments (got {})",
        ids.len()
    );
    let client = ctx.build_client()?;
    let value = api::comment::attach_comment(&client, comment_id, &targets)
        .await
        .context("failed to attach to comment")?;
    ctx.output.render(&value)?;
    Ok(())
}

/// Detach a single object from a comment.
async fn detach(ctx: &CommandContext<'_>, comment_id: &str, target_id: &str) -> Result<()> {
    let client = ctx.build_client()?;
    let value = api::comment::detach_comment(&client, comment_id, target_id)
        .await
        .context("failed to detach from comment")?;
    ctx.output.render(&value)?;
    Ok(())
}

/// Trim and drop blank entries from a `--target-ids` list.
fn clean_target_ids(target_ids: &[String]) -> Vec<String> {
    target_ids
        .iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        AssignTarget, clean_target_ids, take_list_summary, validate_container_entity_type,
        validate_entity_type,
    };
    use serde_json::json;

    #[test]
    fn list_summary_moves_counts_and_limits_to_one_line() {
        let mut value = json!({
            "threads": [{"id": "c1"}],
            "counts": {"all": 3, "open": 2},
            "limits": {"root_threads_used": 3, "root_threads_max": 100},
            "total": 3
        });
        let summary = take_list_summary(&mut value).expect("summary");
        assert!(
            summary.contains("all=3") && summary.contains("open=2"),
            "{summary}"
        );
        assert!(summary.contains("threads: 3 of 100"), "{summary}");
        assert!(value.get("counts").is_none() && value.get("limits").is_none());
        assert_eq!(value["total"], json!(3));

        let mut unlimited = json!({"limits": {"root_threads_used": 5, "root_threads_max": null}});
        assert_eq!(
            take_list_summary(&mut unlimited).as_deref(),
            Some("threads: 5 of unlimited")
        );

        let mut flat = json!({"comments": []});
        assert!(take_list_summary(&mut flat).is_none());
        assert!(flat.get("comments").is_some());
    }

    #[test]
    fn assign_target_maps_each_form() {
        let me = AssignTarget {
            assign_self: true,
            ..AssignTarget::default()
        };
        assert!(matches!(
            me.to_assignee(),
            Ok(fastio_cli::api::comment::Assignee::Caller)
        ));
        let user = AssignTarget {
            user_id: Some("1000000000000000002".to_owned()),
            agent_name: Some("helper".to_owned()),
            credential_type: Some("oauth".to_owned()),
            ..AssignTarget::default()
        };
        assert!(matches!(
            user.to_assignee(),
            Ok(fastio_cli::api::comment::Assignee::User { agent: Some(_), .. })
        ));
        assert!(AssignTarget::default().to_assignee().is_err());
    }

    #[test]
    fn clean_target_ids_trims_and_drops_blanks() {
        let input = vec![
            " a ".to_owned(),
            String::new(),
            "b".to_owned(),
            "   ".to_owned(),
        ];
        assert_eq!(
            clean_target_ids(&input),
            vec!["a".to_owned(), "b".to_owned()]
        );
    }

    /// THE REGRESSION THIS PIN EXISTS FOR.
    ///
    /// `fileshare` is the platform's third comment entity type. An allow-list
    /// that omits it makes the CLI reject client-side a call the platform
    /// accepts, and such a list goes stale silently when the platform grows a
    /// member — which is exactly why asserting on membership here is not enough
    /// on its own; the clap `value_parser` in `cli.rs` is a SECOND enumeration
    /// of the same set and rejects first.
    #[test]
    fn node_scoped_routes_accept_all_three_entity_types() {
        for ty in ["workspace", "share", "fileshare"] {
            assert!(
                validate_entity_type(ty).is_ok(),
                "node-scoped routes must accept '{ty}'"
            );
        }
        assert!(validate_entity_type("org").is_err());
    }

    /// The container listing has no node segment, and the published API docs say
    /// a File Share comment "always targets a specific node, never the whole
    /// container". So the two allow-lists must NOT be the same list — adding
    /// `fileshare` to a single shared one would trade a false rejection for a
    /// request the platform does not serve.
    #[test]
    fn container_route_rejects_fileshare_with_actionable_advice() {
        assert!(validate_container_entity_type("workspace").is_ok());
        assert!(validate_container_entity_type("share").is_ok());

        let err = validate_container_entity_type("fileshare")
            .expect_err("fileshare has no container-scoped listing");
        let msg = err.to_string();
        assert!(
            msg.contains("--node-id"),
            "the error must point at the working alternative, got: {msg}"
        );
    }
}
