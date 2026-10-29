//! `fastio suggestion *` — suggested edits on notes.
//!
//! A suggestion is a root comment with `kind=suggestion`, anchored to a quoted
//! passage, proposing a replacement (or a deletion). Replies are ordinary
//! comment replies; state changes go through the suggestion routes.

use anyhow::{Context, Result};
use colored::Colorize;

use super::CommandContext;
use super::comment::{QuoteArgs, quote_reference, render_list, reply};
use crate::cli::SuggestionCommands;
use fastio_cli::api;

/// Execute a suggestion subcommand.
#[allow(clippy::too_many_lines)]
pub async fn execute(command: SuggestionCommands, ctx: &CommandContext<'_>) -> Result<()> {
    match command {
        SuggestionCommands::Create {
            node_id,
            entity_type,
            entity_id,
            quote,
            prefix,
            suffix,
            occurrence,
            replacement,
            delete,
            note,
        } => {
            validate_node_entity(&entity_type, &entity_id, &node_id)?;
            let change = change_from_flags(replacement.as_deref(), delete)?
                .context("give the replacement with --replacement, or pass --delete")?;
            let quote = QuoteArgs {
                exact: quote,
                prefix,
                suffix,
                occurrence,
            };
            let client = ctx.build_client()?;
            let reference =
                quote_reference(&client, &entity_type, &entity_id, &node_id, &quote).await?;
            let value = api::comment::add_suggestion(
                &client,
                &api::comment::AddSuggestionParams {
                    entity_type: &entity_type,
                    entity_id: &entity_id,
                    node_id: &node_id,
                    reference: &reference,
                    change,
                    note: note.as_deref(),
                },
            )
            .await
            .context("failed to create suggestion")?;
            if api::comment::suggestion_create_was_downgraded(&value) {
                eprintln!(
                    "{} the server created a plain comment, not a suggestion: it does not \
                     support suggestions yet",
                    "warning:".yellow().bold()
                );
            }
            ctx.output.render(&value)?;
            Ok(())
        }
        SuggestionCommands::List {
            node_id,
            entity_type,
            entity_id,
            status,
            for_me,
            threads,
            sort,
            limit,
            offset,
        } => {
            anyhow::ensure!(!entity_id.trim().is_empty(), "entity ID must not be empty");
            let filters = api::comment::CommentListFilters {
                status: status.as_deref(),
                for_me: for_me.as_deref(),
                kind: Some("suggestion"),
                include_suggestions: false,
                threads,
            };
            let client = ctx.build_client()?;
            let value = if let Some(node_id) = node_id.as_deref() {
                validate_node_entity(&entity_type, &entity_id, node_id)?;
                api::comment::list_comments(
                    &client,
                    &api::comment::ListCommentsParams {
                        entity_type: &entity_type,
                        entity_id: &entity_id,
                        node_id,
                        sort: sort.as_deref(),
                        limit,
                        offset,
                        filters,
                    },
                )
                .await
            } else {
                anyhow::ensure!(
                    super::comment::CONTAINER_ENTITY_TYPES.contains(&entity_type.as_str()),
                    "listing across a whole {entity_type} is not supported: pass the note's node ID"
                );
                api::comment::list_all_comments(
                    &client,
                    &entity_type,
                    &entity_id,
                    sort.as_deref(),
                    limit,
                    offset,
                    &filters,
                )
                .await
            }
            .context("failed to list suggestions")?;
            render_list(ctx, value)
        }
        SuggestionCommands::Reply {
            comment_id,
            text,
            node_id,
            entity_type,
            entity_id,
            reopen,
        } => {
            validate_node_entity(&entity_type, &entity_id, &node_id)?;
            reply(
                ctx,
                &api::comment::AddCommentParams {
                    entity_type: &entity_type,
                    entity_id: &entity_id,
                    node_id: &node_id,
                    body: &text,
                    parent_id: Some(&comment_id),
                    reference: None,
                    properties: None,
                    target_id: None,
                    target_ids: None,
                    reopen,
                },
            )
            .await
        }
        SuggestionCommands::Edit {
            comment_id,
            replacement,
            delete,
            note,
        } => {
            let change = change_from_flags(replacement.as_deref(), delete)?;
            let client = ctx.build_client()?;
            let value =
                api::comment::edit_suggestion(&client, &comment_id, change, note.as_deref())
                    .await
                    .context("failed to edit suggestion")?;
            ctx.output.render(&value)?;
            Ok(())
        }
        SuggestionCommands::Withdraw { comment_id } => {
            let client = ctx.build_client()?;
            let value = api::comment::withdraw_suggestion(&client, &comment_id)
                .await
                .context("failed to withdraw suggestion")?;
            ctx.output.render(&value)?;
            Ok(())
        }
        SuggestionCommands::Reject { comment_id } => {
            let client = ctx.build_client()?;
            let value = api::comment::reject_suggestion(&client, &comment_id)
                .await
                .context("failed to reject suggestion")?;
            ctx.output.render(&value)?;
            Ok(())
        }
        SuggestionCommands::Accept { comment_id } => {
            let client = ctx.build_client()?;
            let value = api::comment::accept_suggestion(
                &client,
                &comment_id,
                api::comment::ACCEPT_RETRY_DELAY,
                |err| {
                    if !ctx.output.quiet {
                        eprintln!("{} {}", "note:".yellow().bold(), retry_note(err));
                    }
                },
            )
            .await
            .context("failed to accept suggestion")?;
            ctx.output.render(&value)?;
            Ok(())
        }
    }
}

/// The one-line note printed before the single automatic accept retry.
///
/// After `suggestion_apply_unknown` the first attempt's edit may already be
/// in the note, so the note must not say it "could not apply"; the repeat is
/// safe because a repeat accept never applies twice.
fn retry_note(err: &fastio_cli::error::CliError) -> String {
    use fastio_cli::error::CliError;
    let secs = api::comment::ACCEPT_RETRY_DELAY.as_secs();
    let (CliError::Api(api) | CliError::MappedApi { api, .. }) = err else {
        return format!("trying the accept again once in {secs} seconds");
    };
    let editor_reason = api
        .details
        .as_deref()
        .and_then(|d| d.get("params"))
        .and_then(|p| p.get("editor_reason"))
        .and_then(serde_json::Value::as_str);
    if api.field_reason() == Some("suggestion_apply_unknown") {
        format!(
            "the accept's outcome is unknown (the edit may already be in the note); asking again \
             once in {secs} seconds, which never applies it twice"
        )
    } else if editor_reason == Some("prior_attempt_unknown") {
        format!(
            "an earlier accept's outcome is unknown (its edit may already be in the note); asking \
             again once in {secs} seconds, which never applies it twice"
        )
    } else {
        format!("the note is busy; trying the accept again once in {secs} seconds")
    }
}

/// The change described by `--replacement` / `--delete` (clap keeps them
/// exclusive). `None` when neither was given.
fn change_from_flags(
    replacement: Option<&str>,
    delete: bool,
) -> Result<Option<api::comment::SuggestionChange<'_>>> {
    match (replacement, delete) {
        (Some(_), true) => anyhow::bail!("--replacement and --delete are mutually exclusive"),
        (Some(text), false) => Ok(Some(api::comment::SuggestionChange::Replace(text))),
        (None, true) => Ok(Some(api::comment::SuggestionChange::Delete)),
        (None, false) => Ok(None),
    }
}

/// Validate the entity type and ids of a node-scoped suggestion route.
fn validate_node_entity(entity_type: &str, entity_id: &str, node_id: &str) -> Result<()> {
    anyhow::ensure!(
        super::comment::VALID_ENTITY_TYPES.contains(&entity_type),
        "invalid entity type '{entity_type}'. Must be one of: workspace, share, fileshare"
    );
    anyhow::ensure!(!entity_id.trim().is_empty(), "entity ID must not be empty");
    anyhow::ensure!(!node_id.trim().is_empty(), "node ID must not be empty");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{change_from_flags, retry_note};
    use fastio_cli::api::comment::SuggestionChange;
    use fastio_cli::error::{ApiError, CliError};

    #[test]
    fn retry_note_never_claims_nothing_was_applied_after_an_unknown_outcome() {
        let refusal = |status: u16, reason: &str| {
            CliError::Api(
                ApiError::new(1, None, "x".to_owned(), status)
                    .with_details(serde_json::json!({ "params": { "reason": reason } })),
            )
        };
        let unknown = retry_note(&refusal(503, "suggestion_apply_unknown"));
        assert!(unknown.contains("may already be in the note"), "{unknown}");
        assert!(!unknown.contains("could not apply"), "{unknown}");
        let busy = retry_note(&refusal(409, "suggestion_needs_editor"));
        assert!(busy.contains("busy"), "{busy}");
        let prior = retry_note(&CliError::Api(
            ApiError::new(1, None, "x".to_owned(), 409).with_details(serde_json::json!({
                "params": {
                    "reason": "suggestion_needs_editor",
                    "editor_reason": "prior_attempt_unknown",
                    "retryable": true
                }
            })),
        ));
        assert!(prior.contains("earlier accept"), "{prior}");
        assert!(!prior.contains("busy"), "{prior}");
    }

    #[test]
    fn change_flags_map_to_the_wire_change() {
        assert!(matches!(
            change_from_flags(Some("new"), false),
            Ok(Some(SuggestionChange::Replace("new")))
        ));
        assert!(matches!(
            change_from_flags(None, true),
            Ok(Some(SuggestionChange::Delete))
        ));
        assert!(matches!(change_from_flags(None, false), Ok(None)));
        assert!(change_from_flags(Some("x"), true).is_err());
    }
}
