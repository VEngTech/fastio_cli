#![allow(clippy::missing_errors_doc)]

/// Comment API endpoints for the Fast.io REST API.
///
/// Maps to endpoints for comment CRUD and reactions on workspace/share files.
use std::collections::HashMap;

use serde_json::Value;

use crate::client::ApiClient;
use crate::error::CliError;

/// Accepted values for the list `status` filter.
pub const COMMENT_STATUS_VALUES: &[&str] = &["open", "resolved", "all"];

/// Accepted values for the list `kind` filter.
pub const COMMENT_KIND_VALUES: &[&str] = &["comment", "suggestion", "all"];

/// Accepted values for the list `for_me` filter: `true` (assigned to me with
/// no agent qualifier, or mentioning me) or `agents` (assigned to me through
/// one of my agents). The server derives membership from the caller's
/// credential; no identity is sent.
pub const COMMENT_FOR_ME_VALUES: &[&str] = &["true", "agents"];

/// Optional thread filters shared by [`list_comments`] and
/// [`list_all_comments`].
///
/// Every field defaults to "not sent", so `CommentListFilters::default()` adds
/// nothing to the request and the listing is byte-identical to one made
/// without filters.
#[derive(Debug, Clone, Copy, Default)]
pub struct CommentListFilters<'a> {
    /// `status`: one of [`COMMENT_STATUS_VALUES`].
    pub status: Option<&'a str>,
    /// `for_me`: one of [`COMMENT_FOR_ME_VALUES`].
    pub for_me: Option<&'a str>,
    /// `kind`: one of [`COMMENT_KIND_VALUES`]. `suggestion` implies
    /// `include_suggestions` server-side.
    pub kind: Option<&'a str>,
    /// Send `include_suggestions=true` (suggested-edit threads are hidden
    /// unless asked for).
    pub include_suggestions: bool,
    /// Send `group=threads`: page by root thread, replies embedded.
    pub threads: bool,
}

/// Add the set [`CommentListFilters`] to a list query. Unset fields add
/// nothing.
fn apply_list_filters(query: &mut HashMap<String, String>, filters: &CommentListFilters<'_>) {
    if let Some(v) = filters.status {
        query.insert("status".to_owned(), v.to_owned());
    }
    if let Some(v) = filters.for_me {
        query.insert("for_me".to_owned(), v.to_owned());
    }
    if let Some(v) = filters.kind {
        query.insert("kind".to_owned(), v.to_owned());
    }
    if filters.include_suggestions {
        query.insert("include_suggestions".to_owned(), "true".to_owned());
    }
    if filters.threads {
        query.insert("group".to_owned(), "threads".to_owned());
    }
}

/// Parameters for [`list_comments`].
pub struct ListCommentsParams<'a> {
    /// Kind of parent entity (`workspace` or `share`).
    pub entity_type: &'a str,
    /// Unique identifier of the parent workspace or share.
    pub entity_id: &'a str,
    /// File/folder node within the entity to list comments for.
    pub node_id: &'a str,
    /// Sort order for results: `asc` or `desc` (server default `asc`). Forwarded
    /// verbatim as the `sort` query param.
    pub sort: Option<&'a str>,
    /// Maximum number of comments to return.
    pub limit: Option<u32>,
    /// Number of comments to skip for pagination.
    pub offset: Option<u32>,
    /// Thread filters; the default sends none.
    pub filters: CommentListFilters<'a>,
}

/// List comments on a specific file.
///
/// `GET /comments/{entity_type}/{entity_id}/{node_id}/`
pub async fn list_comments(
    client: &ApiClient,
    params: &ListCommentsParams<'_>,
) -> Result<Value, CliError> {
    let mut query = HashMap::new();
    if let Some(v) = params.sort {
        query.insert("sort".to_owned(), v.to_owned());
    }
    if let Some(l) = params.limit {
        query.insert("limit".to_owned(), l.to_string());
    }
    if let Some(o) = params.offset {
        query.insert("offset".to_owned(), o.to_string());
    }
    apply_list_filters(&mut query, &params.filters);
    let path = format!(
        "/comments/{}/{}/{}/",
        urlencoding::encode(params.entity_type),
        urlencoding::encode(params.entity_id),
        urlencoding::encode(&crate::opaque_id::canonicalize(params.node_id)),
    );
    if query.is_empty() {
        client.get(&path).await
    } else {
        client.get_with_params(&path, &query).await
    }
}

/// Parameters for [`add_comment`].
///
/// `body` (and optionally `parent_id`) are the only fields a plain comment or
/// reply needs; the remaining fields carry the optional create extensions
/// documented in comments.txt:
///
/// - `reference` — an anchoring reference object (see Reference Anchoring).
/// - `properties` — arbitrary key-value metadata.
/// - `target_id` / `target_ids` — inline attachment(s) on a **new** comment
///   (single or batch, ≤25). The server ignores these on an update; the
///   command/MCP layer only sends them on create.
pub struct AddCommentParams<'a> {
    /// Kind of parent entity (`workspace` or `share`).
    pub entity_type: &'a str,
    /// Unique identifier of the parent workspace or share.
    pub entity_id: &'a str,
    /// File/folder node the comment is anchored to.
    pub node_id: &'a str,
    /// Comment text content.
    pub body: &'a str,
    /// Parent comment ID for a single-level threaded reply.
    pub parent_id: Option<&'a str>,
    /// Optional anchoring reference (JSON object) into a file position.
    pub reference: Option<&'a Value>,
    /// Optional arbitrary key-value metadata (JSON object).
    pub properties: Option<&'a Value>,
    /// Inline single attachment (new comment only).
    pub target_id: Option<&'a str>,
    /// Inline multiple attachments (new comment only, ≤25).
    pub target_ids: Option<&'a [String]>,
    /// On a reply, also reopen the resolved thread (`reopen=true`, applied in
    /// the same transaction). A reply never reopens on its own.
    pub reopen: bool,
}

/// Build the request body for [`add_comment`].
///
/// Extracted as a pure function so the body construction is testable without a
/// network round-trip. Only set fields are emitted, matching the server's
/// "omit to leave unset" convention.
fn build_add_comment_body(params: &AddCommentParams<'_>) -> Value {
    let mut body = serde_json::json!({ "body": params.body });
    if let Some(parent_id) = params.parent_id {
        body["parent_id"] = Value::String(parent_id.to_owned());
    }
    if let Some(reference) = params.reference {
        body["reference"] = reference.clone();
    }
    if let Some(properties) = params.properties {
        body["properties"] = properties.clone();
    }
    if let Some(target_id) = params.target_id {
        body["target_id"] = Value::String(target_id.to_owned());
    }
    if let Some(target_ids) = params.target_ids {
        body["target_ids"] = serde_json::json!(target_ids);
    }
    if params.reopen {
        body["reopen"] = Value::Bool(true);
    }
    body
}

/// Maximum comment `body` length, in **characters (code points)** — the
/// platform's authoritative unit, confirmed after its fix `c6eb446b38`.
///
/// Over-limit is a **hard rejection**, never truncation: `162417` on create,
/// `164797` on update.
///
/// **RESOLVED 2026-08-07 — the deploy window this warned about is closed.**
/// Measured at the boundary: 200 CJK (600 B) and 500 CJK (1 500 B)
/// accepted, 501 rejected with `166910` reporting *"has 501 characters"*. The
/// server counts characters and says so. The byte-window caveat that stood here
/// was removed from the user-facing hints the same day.
///
/// History, because the reasoning outlived the window: the server was still
/// byte-counting on 2026-08-06 (`strlen` against a `varchar(8192)` CHARACTER
/// column), so a CJK or emoji author was refused sooner than this bound allows.
/// It could never make this check wrongly refuse, because `chars <= bytes` for
/// every string means a character bound is at worst lenient against a byte
/// server. That framing is why this stayed correct across the platform's
/// reversal, and it is the durable half.
///
/// **The 500-limit on visible text is deliberately NOT checked here** — it
/// discounts mention markup, and this crate has no mention grammar, so it cannot
/// be computed correctly. The server owns it (`166910`).
///
/// That discount is also **conditional**, which only widens the gap: measured
/// 2026-08-10 with a pair differing solely by a code fence, 460 visible
/// characters plus 76 characters of UNFENCED `@[file:…]` markup was accepted,
/// while the same 460 plus the SAME markup inside a fence was rejected
/// (`166910`). Computing the visible length therefore needs the server's fence
/// rules on top of its mention grammar — two things this crate has neither of.
/// Treat `166910` as the authority and read the count from its message.
pub const COMMENT_BODY_MAX_CHARS: usize = 8192;

/// Validate a comment body before sending it.
///
/// See [`COMMENT_BODY_MAX_CHARS`] for the unit and for why the 500-limit on
/// visible text is left to the server.
///
/// # Errors
/// [`CliError::Parse`] when the body is blank after trimming, or longer than
/// [`COMMENT_BODY_MAX_CHARS`] characters.
fn validate_comment_body(text: &str) -> Result<(), CliError> {
    if text.trim().is_empty() {
        return Err(CliError::Parse("comment body must not be empty".to_owned()));
    }
    let chars = text.chars().count();
    if chars > COMMENT_BODY_MAX_CHARS {
        return Err(CliError::Parse(format!(
            "comment body must be at most {COMMENT_BODY_MAX_CHARS} characters (got {chars})"
        )));
    }
    Ok(())
}

/// Add a comment to a file (or reply to one via `parent_id`).
///
/// `POST /comments/{entity_type}/{entity_id}/{node_id}/`
pub async fn add_comment(
    client: &ApiClient,
    params: &AddCommentParams<'_>,
) -> Result<Value, CliError> {
    validate_comment_body(params.body)?;
    let body = build_add_comment_body(params);
    let path = format!(
        "/comments/{}/{}/{}/",
        urlencoding::encode(params.entity_type),
        urlencoding::encode(params.entity_id),
        urlencoding::encode(&crate::opaque_id::canonicalize(params.node_id)),
    );
    client.post_json(&path, &body).await
}

/// Edit an existing comment by ID.
///
/// `POST /comments/{comment_id}/update/`
/// Author-only. Works for every comment surface — workspace, share, node, and
/// File Share. The edit cannot move the comment; entity, scope, and threading
/// are immutable.
pub async fn update_comment(
    client: &ApiClient,
    comment_id: &str,
    text: &str,
) -> Result<Value, CliError> {
    validate_comment_body(text)?;
    let body = serde_json::json!({ "body": text });
    let path = format!("/comments/{}/update/", urlencoding::encode(comment_id));
    client.post_json(&path, &body).await
}

/// Delete a comment.
///
/// `DELETE /comments/{comment_id}/delete/`
pub async fn delete_comment(client: &ApiClient, comment_id: &str) -> Result<Value, CliError> {
    let path = format!("/comments/{}/delete/", urlencoding::encode(comment_id),);
    client.delete(&path).await
}

/// Get comment details.
///
/// `GET /comments/{comment_id}/details/`
pub async fn get_comment_details(client: &ApiClient, comment_id: &str) -> Result<Value, CliError> {
    let path = format!("/comments/{}/details/", urlencoding::encode(comment_id),);
    client.get(&path).await
}

/// List all comments across a workspace or share.
///
/// `GET /comments/{entity_type}/{entity_id}/`
pub async fn list_all_comments(
    client: &ApiClient,
    entity_type: &str,
    entity_id: &str,
    sort: Option<&str>,
    limit: Option<u32>,
    offset: Option<u32>,
    filters: &CommentListFilters<'_>,
) -> Result<Value, CliError> {
    let mut params = HashMap::new();
    if let Some(v) = sort {
        params.insert("sort".to_owned(), v.to_owned());
    }
    if let Some(l) = limit {
        params.insert("limit".to_owned(), l.to_string());
    }
    if let Some(o) = offset {
        params.insert("offset".to_owned(), o.to_string());
    }
    apply_list_filters(&mut params, filters);
    let path = format!(
        "/comments/{}/{}/",
        urlencoding::encode(entity_type),
        urlencoding::encode(entity_id),
    );
    if params.is_empty() {
        client.get(&path).await
    } else {
        client.get_with_params(&path, &params).await
    }
}

/// Add an emoji reaction to a comment.
///
/// `POST /comments/{comment_id}/reactions/`
pub async fn add_reaction(
    client: &ApiClient,
    comment_id: &str,
    emoji: &str,
) -> Result<Value, CliError> {
    let body = serde_json::json!({ "emoji": emoji });
    let path = format!("/comments/{}/reactions/", urlencoding::encode(comment_id));
    client.post_json(&path, &body).await
}

/// Remove an emoji reaction from a comment.
///
/// `DELETE /comments/{comment_id}/reactions/`
pub async fn remove_reaction(client: &ApiClient, comment_id: &str) -> Result<Value, CliError> {
    let path = format!("/comments/{}/reactions/", urlencoding::encode(comment_id));
    client.delete(&path).await
}

/// Bulk-delete multiple comments.
///
/// `POST /comments/bulk/delete/`
pub async fn bulk_delete_comments(
    client: &ApiClient,
    comment_ids: &[String],
) -> Result<Value, CliError> {
    let body = serde_json::json!({ "comment_ids": comment_ids });
    client.post_json("/comments/bulk/delete/", &body).await
}

// ─── Comment Attachments ──────────────────────────────────────────────────────

/// Selector for which object(s) to attach to a comment.
///
/// Mirrors the server's `target_id` (single) / `target_ids` (batch, ≤25) body
/// fields on `POST /comments/{comment_id}/attachments/`. The mutual exclusion
/// (and the "at least one" requirement) is enforced by the CLI/MCP layer.
pub enum CommentAttachTargets<'a> {
    /// Attach a single object (`target_id`).
    Single(&'a str),
    /// Attach multiple objects (`target_ids`, ≤25 per comment).
    Multiple(&'a [String]),
}

/// Build the request body for [`attach_comment`].
///
/// Extracted as a pure function so the body construction is testable without a
/// network round-trip.
fn build_attach_body(targets: &CommentAttachTargets<'_>) -> Value {
    match targets {
        CommentAttachTargets::Single(id) => serde_json::json!({ "target_id": id }),
        CommentAttachTargets::Multiple(ids) => serde_json::json!({ "target_ids": ids }),
    }
}

/// List the objects attached to a comment (hydrated, access-gated).
///
/// `GET /comments/{comment_id}/attachments/`
pub async fn list_comment_attachments(
    client: &ApiClient,
    comment_id: &str,
) -> Result<Value, CliError> {
    let path = format!("/comments/{}/attachments/", urlencoding::encode(comment_id),);
    client.get(&path).await
}

/// Attach one object (`target_id`) or many (`target_ids`) to a comment.
///
/// `POST /comments/{comment_id}/attachments/`. Idempotent (already-attached
/// objects are skipped) and atomic (a partial-batch failure attaches nothing);
/// author-only — the server enforces it. Returns the full updated hydrated
/// attachment list.
pub async fn attach_comment(
    client: &ApiClient,
    comment_id: &str,
    targets: &CommentAttachTargets<'_>,
) -> Result<Value, CliError> {
    let body = build_attach_body(targets);
    let path = format!("/comments/{}/attachments/", urlencoding::encode(comment_id),);
    client.post_json(&path, &body).await
}

/// Build the request body for [`detach_comment`].
fn build_detach_body(target_id: &str) -> Value {
    serde_json::json!({ "target_id": target_id })
}

/// Detach a single object from a comment by its `target_id`.
///
/// `POST /comments/{comment_id}/attachments/detach/`. Idempotent — detaching an
/// object that is not attached returns success with `removed: false`. Returns
/// the updated hydrated attachment list.
pub async fn detach_comment(
    client: &ApiClient,
    comment_id: &str,
    target_id: &str,
) -> Result<Value, CliError> {
    let body = build_detach_body(target_id);
    let path = format!(
        "/comments/{}/attachments/detach/",
        urlencoding::encode(comment_id),
    );
    client.post_json(&path, &body).await
}

// ─── Thread state: resolve / reopen / assign ──────────────────────────────────

/// `/comments/{comment_id}/{action}/`, with the id in its dash-free canonical
/// form (always valid in a path, while some surfaces refuse the dashed one).
fn comment_action_path(comment_id: &str, action: &str) -> String {
    format!(
        "/comments/{}/{action}/",
        urlencoding::encode(&crate::opaque_id::canonicalize(comment_id))
    )
}

/// Re-hint a reasonless `404` from one of the thread-state or suggestion
/// routes.
///
/// These routes are newer than some deployments, and from here an unknown
/// route and an unknown comment look the same, so the generic not-found hint
/// would send the caller chasing the id alone. A `404` that carries a
/// `reason` keeps its own hint.
fn map_new_route_error(err: CliError) -> CliError {
    match err {
        CliError::Api(api) if api.http_status == 404 && api.field_reason().is_none() => {
            CliError::MappedApi {
                api,
                hint: Some(crate::error::HINT_COMMENT_ROUTE_NOT_FOUND),
            }
        }
        other => other,
    }
}

/// Body for resolve / reopen: `{}` or `{if_state_version}`.
fn build_state_body(if_state_version: Option<u64>) -> Value {
    match if_state_version {
        Some(v) => serde_json::json!({ "if_state_version": v }),
        None => serde_json::json!({}),
    }
}

/// Resolve a comment thread (root comments only; idempotent).
///
/// `POST /comments/{comment_id}/resolve/`. With `if_state_version`, the
/// server refuses (`409 state_version_conflict`) when the thread moved on.
pub async fn resolve_comment(
    client: &ApiClient,
    comment_id: &str,
    if_state_version: Option<u64>,
) -> Result<Value, CliError> {
    client
        .post_json(
            &comment_action_path(comment_id, "resolve"),
            &build_state_body(if_state_version),
        )
        .await
        .map_err(map_new_route_error)
}

/// Reopen a resolved comment thread (root comments only; idempotent).
///
/// `POST /comments/{comment_id}/reopen/`. Same precondition as
/// [`resolve_comment`].
pub async fn reopen_comment(
    client: &ApiClient,
    comment_id: &str,
    if_state_version: Option<u64>,
) -> Result<Value, CliError> {
    client
        .post_json(
            &comment_action_path(comment_id, "reopen"),
            &build_state_body(if_state_version),
        )
        .await
        .map_err(map_new_route_error)
}

/// An agent qualifier on an explicit assignee: "this person, via this agent".
#[derive(Debug, Clone, Copy)]
pub struct AgentQualifier<'a> {
    /// The agent's display name.
    pub agent_name: &'a str,
    /// The agent's credential type: `oauth`, `api_key` or `platform`.
    pub credential_type: &'a str,
    /// The specific credential, when known.
    pub credential_id: Option<&'a str>,
}

/// Accepted `credential_type` values for an [`AgentQualifier`].
pub const ASSIGNEE_CREDENTIAL_TYPES: &[&str] = &["oauth", "api_key", "platform"];

/// Who a thread is assigned to.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum Assignee<'a> {
    /// The caller (`assignee: "self"`); the server derives the user, and the
    /// agent qualifier when the caller is an agent credential.
    Caller,
    /// Ripley (`assignee: "ripley"`), on behalf of the caller.
    Ripley,
    /// A specific user, optionally through one of their agents.
    User {
        /// The assignee's user id.
        user_id: &'a str,
        /// Optional agent qualifier.
        agent: Option<AgentQualifier<'a>>,
    },
}

/// Body for assign; `None` clears the assignee.
fn build_assign_body(assignee: Option<&Assignee<'_>>) -> Value {
    match assignee {
        None => serde_json::json!({ "clear": true }),
        Some(Assignee::Caller) => serde_json::json!({ "assignee": "self" }),
        Some(Assignee::Ripley) => serde_json::json!({ "assignee": "ripley" }),
        Some(Assignee::User { user_id, agent }) => {
            let mut body = serde_json::json!({ "assignee_user_id": user_id });
            if let Some(agent) = agent {
                let mut qualifier = serde_json::json!({
                    "agent_name": agent.agent_name,
                    "credential_type": agent.credential_type,
                });
                if let Some(id) = agent.credential_id {
                    qualifier["credential_id"] = Value::String(id.to_owned());
                }
                body["assignee_agent"] = qualifier;
            }
            body
        }
    }
}

/// Assign a comment thread (root comments only).
///
/// `POST /comments/{comment_id}/assign/`
pub async fn assign_comment(
    client: &ApiClient,
    comment_id: &str,
    assignee: &Assignee<'_>,
) -> Result<Value, CliError> {
    client
        .post_json(
            &comment_action_path(comment_id, "assign"),
            &build_assign_body(Some(assignee)),
        )
        .await
        .map_err(map_new_route_error)
}

/// Clear a comment thread's assignee.
///
/// `POST /comments/{comment_id}/assign/` with `{clear: true}`.
pub async fn unassign_comment(client: &ApiClient, comment_id: &str) -> Result<Value, CliError> {
    client
        .post_json(
            &comment_action_path(comment_id, "assign"),
            &build_assign_body(None),
        )
        .await
        .map_err(map_new_route_error)
}

// ─── Quote anchors ────────────────────────────────────────────────────────────

/// Maximum length of a quote's `exact` text, in UTF-16 code units (the
/// anchor's text space).
pub const QUOTE_EXACT_MAX: usize = 500;

/// Maximum length of a quote's `prefix` / `suffix`, in UTF-16 code units.
pub const QUOTE_CONTEXT_MAX: usize = 100;

/// Maximum size of the whole `reference` object, in encoded JSON bytes.
pub const REFERENCE_MAX_BYTES: usize = 2048;

/// The one-line warning for a quote that does not occur in the note's stored
/// text. The anchor is still sent: the stored text can lag the live note.
pub const QUOTE_NOT_FOUND_WARNING: &str = "the quoted text was not found in the note's stored text, which can lag \
     the live note by a few minutes; anchoring it without surrounding context";

/// Characters that may render differently from how they are written in
/// markdown. Derived context stops before any of them.
///
/// `(` and `)` are here because a link's destination (`](https://…)`) is not
/// rendered, and a prefix walking BACKWARDS from text after a link would
/// otherwise cross it before reaching the `]`.
const MARKDOWN_SIGNIFICANT: &[char] = &[
    '\\', '*', '_', '`', '[', ']', '<', '>', '#', '~', '|', '&', '!', '(', ')',
];

/// A quote anchor: the quoted text plus the text immediately around it.
///
/// All three are in the note's rendered plain-text space. Empty context is
/// valid; it just narrows nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuoteAnchor {
    /// The quoted text.
    pub exact: String,
    /// Text immediately before the quote.
    pub prefix: String,
    /// Text immediately after the quote.
    pub suffix: String,
}

/// How a [`QuoteAnchor`] relates to the note text it was checked against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum QuoteMatch {
    /// `prefix + exact + suffix` occurs exactly once.
    Unique,
    /// The quote does not occur in the note's stored text. The anchor is
    /// still valid to send; warn with [`QUOTE_NOT_FOUND_WARNING`].
    NotFound,
    /// The note text could not be read here, so nothing was checked (both
    /// context strings were given explicitly).
    Unchecked,
}

/// What the caller asked to anchor.
#[derive(Debug, Clone, Copy)]
pub struct QuoteRequest<'a> {
    /// The quoted text.
    pub exact: &'a str,
    /// Explicit text before the quote; derived when `None`.
    pub prefix: Option<&'a str>,
    /// Explicit text after the quote; derived when `None`.
    pub suffix: Option<&'a str>,
    /// 1-based choice among repeated matches.
    pub occurrence: Option<usize>,
}

/// Length in UTF-16 code units, the anchor's unit of measure.
fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Every byte offset at which `needle` starts in `haystack`, overlapping
/// matches included. An empty needle matches nowhere.
fn match_starts(haystack: &str, needle: &str) -> Vec<usize> {
    let mut starts = Vec::new();
    if needle.is_empty() {
        return starts;
    }
    let mut from = 0;
    while let Some(found) = haystack.get(from..).and_then(|rest| rest.find(needle)) {
        let at = from + found;
        starts.push(at);
        // Advance by one character, not by the needle, so overlapping
        // matches are counted too.
        let step = haystack
            .get(at..)
            .and_then(|rest| rest.chars().next())
            .map_or(1, char::len_utf8);
        from = at + step;
    }
    starts
}

/// Whether `c` ends a derived context window. `outer` is the next character
/// further away from the quote, if any.
fn stops_window(c: char, outer: Option<char>) -> bool {
    c == '\n'
        || c == '\r'
        || c == '\t'
        || MARKDOWN_SIGNIFICANT.contains(&c)
        // A run of spaces may be collapsed when rendered.
        || (c == ' ' && outer.is_some_and(char::is_whitespace))
}

/// `len` when `s` continues with whitespace or ends there — a block marker
/// must be followed by one of the two.
fn followed_by_space(s: &str, len: usize) -> Option<usize> {
    s.get(len..)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t']))
        .then_some(len)
}

/// Length of a list-item marker at the start of `s`: a bullet (`*`, `-`, `+`)
/// or an ordered marker (`1.` / `1)`, at most 9 digits).
fn list_marker_len(s: &str) -> Option<usize> {
    if s.starts_with(['*', '-', '+']) {
        return followed_by_space(s, 1);
    }
    let digits = s.bytes().take_while(u8::is_ascii_digit).count();
    if (1..=9).contains(&digits) && s.get(digits..).is_some_and(|r| r.starts_with(['.', ')'])) {
        return followed_by_space(s, digits + 1);
    }
    None
}

/// Length of an ATX heading's `#`-run (1 to 6) at the start of `s`.
fn heading_marker_len(s: &str) -> Option<usize> {
    let hashes = s.bytes().take_while(|&b| b == b'#').count();
    if (1..=6).contains(&hashes) {
        followed_by_space(s, hashes)
    } else {
        None
    }
}

/// Length of a task-list box (`[ ]`, `[x]`, `[X]`) at the start of `s`.
fn task_box_len(s: &str) -> Option<usize> {
    if s.starts_with("[ ]") || s.starts_with("[x]") || s.starts_with("[X]") {
        followed_by_space(s, 3)
    } else {
        None
    }
}

/// Byte length of what a line's start does not render, following the `CommonMark`
/// line-start structure:
///
/// - indentation;
/// - then CONTAINER markers, which nest in any order: blockquote `>` and list
///   items (`- `, `* `, `+ `, `1. `, `1) `), so `> - `, `- > ` and `- - `
///   (a list inside a list item) are all structure; a task box (`[ ] `)
///   counts only directly after a list marker, and ends the structure;
/// - then at most one LEAF marker, an ATX heading `#`-run, after which
///   everything is text (`# # Title` renders as `# Title`).
///
/// Each marker takes the whitespace after it. Erring towards stripping is
/// safe here: whatever remains is still a suffix of the rendered line.
fn line_lead_len(line: &str) -> usize {
    let skip_space = |at: usize| {
        at + line.get(at..).map_or(0, |rest| {
            rest.len() - rest.trim_start_matches([' ', '\t']).len()
        })
    };
    let mut at = skip_space(0);
    loop {
        let rest = line.get(at..).unwrap_or_default();
        if rest.starts_with('>') {
            at = skip_space(at + 1);
        } else if let Some(len) = list_marker_len(rest) {
            at = skip_space(at + len);
            if let Some(len) = line.get(at..).and_then(task_box_len) {
                return skip_space(at + len);
            }
        } else if let Some(len) = heading_marker_len(rest) {
            return skip_space(at + len);
        } else {
            return at;
        }
    }
}

/// Why a context walk ended.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WindowStop {
    /// A line boundary or the edge of the text.
    LineEdge,
    /// A character that may render differently (see [`stops_window`]).
    Char(char),
    /// The [`QUOTE_CONTEXT_MAX`] cap.
    Cap,
}

/// Whitespace beside a table pipe or a heading `#` is padding that does not
/// render, so a window that stopped on one drops the whitespace next to it.
/// (Whitespace beside inline markers such as `**` or `]` DOES render, and is
/// kept.)
fn stop_pads_whitespace(stop: WindowStop) -> bool {
    matches!(stop, WindowStop::Char('|' | '#'))
}

/// Derive the prefix: up to [`QUOTE_CONTEXT_MAX`] units of the same line
/// immediately before `exact_start`, stopping before anything that may render
/// differently from its markdown.
///
/// When everything left between the line start and the window is
/// indentation and block markers (`# `, `> `, `- `, `1. `, `- [ ] `), the
/// window is treated as starting the line and those markers are dropped, so a
/// heading or list item yields its rendered text, not its syntax.
fn derive_prefix(text: &str, exact_start: usize) -> String {
    let before = text.get(..exact_start).unwrap_or_default();
    let mut chars = before.char_indices().rev().peekable();
    let mut start = exact_start;
    let mut units = 0;
    let mut stop = WindowStop::LineEdge;
    while let Some((i, c)) = chars.next() {
        let outer = chars.peek().map(|&(_, o)| o);
        if c == '\n' || c == '\r' {
            break;
        }
        if stops_window(c, outer) {
            stop = WindowStop::Char(c);
            break;
        }
        if units + c.len_utf16() > QUOTE_CONTEXT_MAX {
            stop = WindowStop::Cap;
            break;
        }
        units += c.len_utf16();
        start = i;
    }
    let line_start = before.rfind(['\n', '\r']).map_or(0, |i| i + 1);
    let line = before.get(line_start..).unwrap_or_default();
    let head_len = start - line_start;
    let lead = line_lead_len(line);
    if lead >= head_len {
        // Only unrendered line-start syntax precedes the window.
        return line.get(lead..).unwrap_or_default().to_owned();
    }
    // A window must not start inside an entity (`amp; ` of `&amp; `, or the
    // `;` closing one): that text is not rendered, the entity renders as one
    // character. Cut to just after the entity, and keep what follows as is —
    // the `#` the walk stopped on was the entity's, not a heading's.
    if let Some(after) = entity_spanning(text, start) {
        return before
            .get(after.min(exact_start)..)
            .unwrap_or_default()
            .to_owned();
    }
    let window = before.get(start..).unwrap_or_default();
    if stop_pads_whitespace(stop) {
        window.trim_start().to_owned()
    } else {
        window.to_owned()
    }
}

/// The end (just past its `;`) of an entity `&[A-Za-z0-9#]{1,10};` that
/// starts before byte `at` and ends at or after it — one a window starting at
/// `at` would cut into. A plain ASCII byte scan of at most 12 bytes before
/// `at`, so it is safe on any text and never slices.
fn entity_spanning(text: &str, at: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    (at.saturating_sub(12)..at).rev().find_map(|amp| {
        if bytes.get(amp) != Some(&b'&') {
            return None;
        }
        let name = entity_name_len(bytes.get(amp + 1..)?);
        let semi = amp + 1 + name;
        (name > 0 && bytes.get(semi) == Some(&b';') && semi >= at).then_some(semi + 1)
    })
}

/// Length of an entity name (`[A-Za-z0-9#]`, at most 10 bytes) at the start
/// of `bytes`.
fn entity_name_len(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .take(10)
        .take_while(|b| b.is_ascii_alphanumeric() || **b == b'#')
        .count()
}

/// One inline link `[text](dest)` found by [`inline_links`].
struct InlineLink {
    /// The `[` that opens the link text, when one was found on its line.
    open: Option<usize>,
    /// The `]` that closes the link text.
    close: usize,
    /// The destination, `(` through `)`. Not rendered.
    dest: std::ops::Range<usize>,
}

/// The longest link destination, `(` through `)`, in bytes. Anything longer
/// is treated as not a link, which can only over-count a quote (a
/// conservative refusal).
const MAX_LINK_DEST_BYTES: usize = 2048;

/// For every `(` whose destination is well formed, the byte index just past
/// its matching `)`: nested parentheses balance (`\)` is skipped), the run
/// holds no newline, space or tab, and it is at most [`MAX_LINK_DEST_BYTES`]
/// long. One linear pass with a stack, so a note full of unclosed `](` costs
/// no more than any other note (scanning ahead from each `(` would be
/// quadratic). Returned as `(open, end)` pairs sorted by `open`, for binary
/// search.
fn destination_ends(bytes: &[u8]) -> Vec<(usize, usize)> {
    let mut ends = Vec::new();
    let mut open_parens: Vec<usize> = Vec::new();
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        match b {
            b'\\' => {
                i += 2;
                continue;
            }
            b'(' => open_parens.push(i),
            b')' => {
                if let Some(open) = open_parens.pop()
                    && i - open < MAX_LINK_DEST_BYTES
                {
                    ends.push((open, i + 1));
                }
            }
            b'\n' | b'\r' | b' ' | b'\t' => open_parens.clear(),
            _ => {}
        }
        i += 1;
    }
    ends.sort_unstable();
    ends
}

/// Every inline link in `text`, in one forward pass, in text order (so their
/// destinations are sorted and disjoint).
///
/// A `]` that is backslash-escaped or inside a backtick code span (tracked by
/// backtick-run parity on each line) opens no destination, and one whose
/// destination is not well formed (see [`destination_ends`]) is not a link:
/// nothing about it is treated as hidden. Only destinations are hidden;
/// autolinks, reference definitions and HTML comments are left as visible
/// text, which can only over-count a quote (a conservative refusal), never
/// pick a wrong one.
fn inline_links(text: &str) -> Vec<InlineLink> {
    let bytes = text.as_bytes();
    let ends = destination_ends(bytes);
    let mut links = Vec::new();
    let mut openers: Vec<usize> = Vec::new();
    let mut in_code = false;
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        match b {
            b'\n' => {
                in_code = false;
                openers.clear();
            }
            b'\\' => {
                i += 2;
                continue;
            }
            b'`' => {
                while bytes.get(i + 1) == Some(&b'`') {
                    i += 1;
                }
                in_code = !in_code;
            }
            b'[' if !in_code => openers.push(i),
            b']' if !in_code => {
                let open = openers.pop();
                if bytes.get(i + 1) == Some(&b'(')
                    && let Ok(at) = ends.binary_search_by_key(&(i + 1), |&(open, _)| open)
                    && let Some(&(_, end)) = ends.get(at)
                {
                    links.push(InlineLink {
                        open,
                        close: i,
                        dest: i + 1..end,
                    });
                    i = end;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
    links
}

/// [`match_starts`] without the matches that touch an inline link
/// destination (see [`inline_links`]). Linear: matches and destinations are
/// both in text order, so one moving pointer filters them.
fn visible_match_starts(text: &str, needle: &str) -> Vec<usize> {
    let links = inline_links(text);
    let mut next = 0;
    match_starts(text, needle)
        .into_iter()
        .filter(|&at| {
            while links.get(next).is_some_and(|l| l.dest.end <= at) {
                next += 1;
            }
            links
                .get(next)
                .is_none_or(|l| l.dest.start >= at + needle.len())
        })
        .collect()
}

/// Derive the suffix: the mirror of [`derive_prefix`] after `exact_end`. When
/// the window reaches the end of its line, or stops on a table pipe or a
/// closing heading `#`, the whitespace at its far end is dropped (it does not
/// render).
fn derive_suffix(text: &str, exact_end: usize) -> String {
    let after = text.get(exact_end..).unwrap_or_default();
    let mut chars = after.char_indices().peekable();
    let mut end = 0;
    let mut units = 0;
    let mut stop = WindowStop::LineEdge;
    while let Some((i, c)) = chars.next() {
        let outer = chars.peek().map(|&(_, o)| o);
        if c == '\n' || c == '\r' {
            break;
        }
        if stops_window(c, outer) {
            stop = WindowStop::Char(c);
            break;
        }
        if units + c.len_utf16() > QUOTE_CONTEXT_MAX {
            stop = WindowStop::Cap;
            break;
        }
        units += c.len_utf16();
        end = i + c.len_utf8();
    }
    let mut window = after.get(..end).unwrap_or_default();
    // Mirror of the prefix rule: never end inside an entity. (`&` is a stop
    // character, so a forward walk cannot normally reach one.)
    if let Some(amp) = window.rfind('&')
        && !window.get(amp..).is_some_and(|tail| tail.contains(';'))
    {
        window = window.get(..amp).unwrap_or_default();
    }
    if stop == WindowStop::LineEdge || stop_pads_whitespace(stop) {
        window.trim_end().to_owned()
    } else {
        window.to_owned()
    }
}

/// Validate the caller-supplied parts of a [`QuoteRequest`].
fn validate_quote_request(req: &QuoteRequest<'_>) -> Result<(), CliError> {
    if req.exact.trim().is_empty() {
        return Err(CliError::Parse(
            "the quote must contain visible text".to_owned(),
        ));
    }
    let exact_len = utf16_len(req.exact);
    if exact_len > QUOTE_EXACT_MAX {
        return Err(CliError::Parse(format!(
            "the quote must be at most {QUOTE_EXACT_MAX} characters (got {exact_len})"
        )));
    }
    for (name, value) in [("prefix", req.prefix), ("suffix", req.suffix)] {
        if let Some(v) = value {
            let len = utf16_len(v);
            if len > QUOTE_CONTEXT_MAX {
                return Err(CliError::Parse(format!(
                    "the {name} must be at most {QUOTE_CONTEXT_MAX} characters (got {len})"
                )));
            }
        }
    }
    if req.occurrence == Some(0) {
        return Err(CliError::Parse(
            "the occurrence is 1-based: pass 1 for the first match".to_owned(),
        ));
    }
    Ok(())
}

/// Build a quote anchor against the note's text, deriving any context not
/// given explicitly, and check that the anchor is not ambiguous.
///
/// `note_text` is the note's stored markdown, or `None` when it cannot be
/// read from this surface (a File Share); then both `prefix` and `suffix`
/// must be explicit and nothing is checked.
///
/// The check: `prefix + exact + suffix` (with any unset side empty) is
/// located in the text.
/// - **No match**: allowed, returned as [`QuoteMatch::NotFound`] with the
///   given context (or `""`), because the stored text can lag the live note —
///   unless the formatting-stripped text shows it more than once (see
///   [`ensure_unambiguous`]), which is refused as ambiguous.
/// - **One match**, or the `occurrence`-th of several: the missing context is
///   derived around it, and the final `prefix + exact + suffix` must occur
///   exactly once, or the anchor is refused.
/// - **Several matches** and no `occurrence`: refused, asking for more context.
///
/// Derived context is the "safe window": at most [`QUOTE_CONTEXT_MAX`] units
/// on the quote's own line, stopping before any markdown-significant
/// character, a tab or a run of spaces, and dropping a line-leading list
/// marker. Inside that window the markdown and its rendered text are the
/// same, so no markdown parser is needed.
///
/// # Errors
/// [`CliError::Parse`] for an invalid request, an out-of-range
/// `occurrence`, or an ambiguous anchor.
pub fn resolve_quote_anchor(
    note_text: Option<&str>,
    req: &QuoteRequest<'_>,
) -> Result<(QuoteAnchor, QuoteMatch), CliError> {
    validate_quote_request(req)?;
    let anchor = |prefix: &str, suffix: &str| QuoteAnchor {
        exact: req.exact.to_owned(),
        prefix: prefix.to_owned(),
        suffix: suffix.to_owned(),
    };
    let Some(text) = note_text else {
        return match (req.prefix, req.suffix, req.occurrence) {
            (Some(prefix), Some(suffix), None) => {
                Ok((anchor(prefix, suffix), QuoteMatch::Unchecked))
            }
            _ => Err(CliError::Parse(
                "this note's text cannot be read here to derive context: pass both --prefix and \
                 --suffix (empty is allowed), and no --occurrence"
                    .to_owned(),
            )),
        };
    };
    let given_prefix = req.prefix.unwrap_or_default();
    let given_suffix = req.suffix.unwrap_or_default();
    let needle = format!("{given_prefix}{}{given_suffix}", req.exact);
    let starts = visible_match_starts(text, &needle);
    let chosen = match (req.occurrence, starts.len()) {
        (None, 0) => {
            ensure_unambiguous(text, given_prefix, req.exact, given_suffix)?;
            return Ok((anchor(given_prefix, given_suffix), QuoteMatch::NotFound));
        }
        (None, 1) => starts.first().copied(),
        (None, n) => {
            return Err(CliError::Parse(format!(
                "the quote occurs {n} times in the note: add more context with --prefix / \
                 --suffix, or pick one with --occurrence N (1 to {n})"
            )));
        }
        (Some(k), n) => {
            // Numbering is only meaningful when the visible matches are the
            // ones a reader counts; formatting can split or join them.
            let rendered =
                match_starts(&approximate_rendered(text), &approximate_rendered(&needle)).len();
            if rendered != n {
                return Err(CliError::Parse(
                    "the note's formatting makes occurrence numbering unreliable here; give \
                     more context with --prefix / --suffix instead of --occurrence"
                        .to_owned(),
                ));
            }
            if k > n {
                return Err(CliError::Parse(format!(
                    "--occurrence {k} is out of range: the quote occurs {n} time(s) in the note"
                )));
            }
            starts.get(k - 1).copied()
        }
    };
    let Some(chosen) = chosen else {
        return Err(CliError::Parse("the quote could not be located".to_owned()));
    };
    let exact_start = chosen + given_prefix.len();
    let exact_end = exact_start + req.exact.len();
    let prefix = req
        .prefix
        .map_or_else(|| derive_prefix(text, exact_start), str::to_owned);
    let suffix = req
        .suffix
        .map_or_else(|| derive_suffix(text, exact_end), str::to_owned);
    ensure_unambiguous(text, &prefix, req.exact, &suffix)?;
    Ok((anchor(&prefix, &suffix), QuoteMatch::Unique))
}

/// Inline formatting characters whose removal approximates the rendered text
/// for the uniqueness check (see [`ensure_unambiguous`]).
const INLINE_FORMATTING: &[char] = &['*', '_', '`', '~'];

/// Decode one entity at the start of `s` (which starts with `&`): `&amp;`,
/// `&lt;`, `&gt;`, `&quot;`, `&apos;`, `&nbsp;`, `&#NN;`, `&#xHH;`. Returns the
/// character and the entity's byte length.
fn decode_entity(s: &str) -> Option<(char, usize)> {
    let bytes = s.as_bytes();
    let name_len = entity_name_len(bytes.get(1..)?);
    if name_len == 0 || bytes.get(1 + name_len) != Some(&b';') {
        return None;
    }
    let semi = 1 + name_len;
    let name = s.get(1..semi)?;
    let decoded = match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        _ => {
            let code = name.strip_prefix('#')?;
            let value = match code.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => code.parse().ok()?,
            };
            char::from_u32(value)?
        }
    };
    Some((decoded, semi + 1))
}

/// An approximation of how `s` reads once rendered, for the uniqueness
/// check: an inline link's brackets and destination removed (exactly the
/// links [`inline_links`] finds, so this agrees with candidate selection),
/// `*`, `_`, backticks and `~` removed, backslash escapes resolved, and common entities
/// decoded. Linear in `s`. It is not a markdown renderer.
fn approximate_rendered(s: &str) -> String {
    let mut skips: Vec<std::ops::Range<usize>> = Vec::new();
    for link in inline_links(s) {
        if let Some(open) = link.open {
            skips.push(open..open + 1);
        }
        skips.push(link.close..link.close + 1);
        skips.push(link.dest);
    }
    skips.sort_unstable_by_key(|r| r.start);
    let mut out = String::with_capacity(s.len());
    let mut chars = s.char_indices().peekable();
    let mut next = 0;
    while let Some((i, c)) = chars.next() {
        while skips.get(next).is_some_and(|r| r.end <= i) {
            next += 1;
        }
        if skips.get(next).is_some_and(|r| r.start <= i) {
            continue;
        }
        match c {
            '\\' => match chars.peek() {
                Some(&(_, escaped)) if escaped.is_ascii_punctuation() => {
                    out.push(escaped);
                    chars.next();
                }
                _ => out.push(c),
            },
            '&' => match s.get(i..).and_then(decode_entity) {
                Some((decoded, len)) => {
                    out.push(decoded);
                    while chars.peek().is_some_and(|&(j, _)| j < i + len) {
                        chars.next();
                    }
                }
                None => out.push(c),
            },
            c if INLINE_FORMATTING.contains(&c) => {}
            _ => out.push(c),
        }
    }
    out
}

/// Refuse an anchor whose `prefix + exact + suffix` occurs more than once.
///
/// Counted twice: in the stored markdown, and in an approximation of the
/// rendered text (see [`approximate_rendered`]). The second count catches a
/// match that the markdown hides but the reader sees (`The **cat** sat.` and
/// `The [cat](url) sat.` both read as `The cat sat.`). Removing characters
/// can only reveal matches, so this is a stricter check, never a looser one.
fn ensure_unambiguous(text: &str, prefix: &str, exact: &str, suffix: &str) -> Result<(), CliError> {
    let needle = format!("{prefix}{exact}{suffix}");
    let raw = visible_match_starts(text, &needle).len();
    let rendered = match_starts(&approximate_rendered(text), &approximate_rendered(&needle)).len();
    let count = raw.max(rendered);
    if count > 1 {
        return Err(CliError::Parse(format!(
            "the quote with its context still occurs {count} times in the note: add more \
             context with a longer --prefix / --suffix"
        )));
    }
    Ok(())
}

impl QuoteAnchor {
    /// The `reference` object for a comment or suggestion create:
    /// `{type: "document", exact, prefix, suffix}`. Offsets are never sent,
    /// and `prefix` / `suffix` are always present (possibly empty).
    ///
    /// # Errors
    /// [`CliError::Parse`] when the encoded object exceeds
    /// [`REFERENCE_MAX_BYTES`].
    pub fn to_reference(&self) -> Result<Value, CliError> {
        let reference = serde_json::json!({
            "type": "document",
            "exact": self.exact,
            "prefix": self.prefix,
            "suffix": self.suffix,
        });
        let bytes = reference.to_string().len();
        if bytes > REFERENCE_MAX_BYTES {
            return Err(CliError::Parse(format!(
                "the quote anchor must encode to at most {REFERENCE_MAX_BYTES} bytes (got {bytes}); \
                 quote a shorter passage"
            )));
        }
        Ok(reference)
    }
}

/// Read the note's stored text for quote anchoring.
///
/// Workspace notes read through [`crate::api::workspace::read_note`], share
/// notes through [`crate::api::workspace::read_note_share`]. A File Share
/// cannot read note text here, so it yields `None`.
pub async fn fetch_quote_text(
    client: &ApiClient,
    entity_type: &str,
    entity_id: &str,
    node_id: &str,
) -> Result<Option<String>, CliError> {
    let value = match entity_type {
        "workspace" => crate::api::workspace::read_note(client, entity_id, node_id, None).await?,
        "share" => crate::api::workspace::read_note_share(client, entity_id, node_id, None).await?,
        _ => return Ok(None),
    };
    value
        .get("content")
        .and_then(Value::as_str)
        .map(|s| Some(s.to_owned()))
        .ok_or_else(|| CliError::Parse("the note response carried no text".to_owned()))
}

/// Read the note (see [`fetch_quote_text`]), resolve the quote anchor (see
/// [`resolve_quote_anchor`]), and build its `reference` object.
pub async fn build_quote_reference(
    client: &ApiClient,
    entity_type: &str,
    entity_id: &str,
    node_id: &str,
    req: &QuoteRequest<'_>,
) -> Result<(Value, QuoteMatch), CliError> {
    // Validate before the read so a bad request costs no round-trip.
    validate_quote_request(req)?;
    let text = fetch_quote_text(client, entity_type, entity_id, node_id).await?;
    let (anchor, matched) = resolve_quote_anchor(text.as_deref(), req)?;
    Ok((anchor.to_reference()?, matched))
}

// ─── Suggested edits ──────────────────────────────────────────────────────────

/// The change a suggestion proposes to its quoted text.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum SuggestionChange<'a> {
    /// Replace the quoted text with this (sent verbatim, never trimmed).
    Replace(&'a str),
    /// Delete the quoted text (`replacement: ""`).
    Delete,
}

impl SuggestionChange<'_> {
    /// The `suggestion` object: `{op, replacement}`.
    fn to_json(self) -> Value {
        match self {
            Self::Replace(text) => serde_json::json!({ "op": "replace", "replacement": text }),
            Self::Delete => serde_json::json!({ "op": "delete", "replacement": "" }),
        }
    }
}

/// Parameters for [`add_suggestion`].
pub struct AddSuggestionParams<'a> {
    /// Kind of parent entity (`workspace`, `share` or `fileshare`).
    pub entity_type: &'a str,
    /// Unique identifier of the parent entity.
    pub entity_id: &'a str,
    /// The note the suggestion targets.
    pub node_id: &'a str,
    /// The quote anchor (see [`QuoteAnchor::to_reference`]).
    pub reference: &'a Value,
    /// The proposed change.
    pub change: SuggestionChange<'a>,
    /// Optional rationale, carried as the root comment `body`.
    pub note: Option<&'a str>,
}

/// Build the request body for [`add_suggestion`]. The rationale rides in
/// `body` and may be empty.
fn build_add_suggestion_body(params: &AddSuggestionParams<'_>) -> Value {
    serde_json::json!({
        "body": params.note.unwrap_or_default(),
        "kind": "suggestion",
        "reference": params.reference,
        "suggestion": params.change.to_json(),
    })
}

/// Create a suggested edit: a root comment with `kind=suggestion`.
///
/// `POST /comments/{entity_type}/{entity_id}/{node_id}/`. Content rules
/// (non-blank replacement, different from the quote, single-line quote) are
/// the server's; its refusals carry `suggestion_invalid` / `anchor_*`.
///
/// Sent without transport replay: a create re-sent after a lost reply would
/// leave a duplicate suggestion on the note.
pub async fn add_suggestion(
    client: &ApiClient,
    params: &AddSuggestionParams<'_>,
) -> Result<Value, CliError> {
    let body = build_add_suggestion_body(params);
    let path = format!(
        "/comments/{}/{}/{}/",
        urlencoding::encode(params.entity_type),
        urlencoding::encode(params.entity_id),
        urlencoding::encode(&crate::opaque_id::canonicalize(params.node_id)),
    );
    client.post_json_no_replay(&path, &body).await
}

/// Whether a create meant as a suggestion came back as something else.
///
/// A server without suggestions ignores `kind` and creates a plain comment.
/// Only a returned `comment` object whose `kind` is not `suggestion` counts;
/// an unexpected response shape is not reported as a downgrade.
#[must_use]
pub fn suggestion_create_was_downgraded(response: &Value) -> bool {
    response
        .get("comment")
        .filter(|c| c.is_object())
        .is_some_and(|c| c.get("kind").and_then(Value::as_str) != Some("suggestion"))
}

/// Build the request body for [`edit_suggestion`].
fn build_edit_suggestion_body(
    change: Option<SuggestionChange<'_>>,
    note: Option<&str>,
) -> Result<Value, CliError> {
    if change.is_none() && note.is_none() {
        return Err(CliError::Parse(
            "nothing to change: give a new replacement, --delete, or a new note".to_owned(),
        ));
    }
    let mut body = serde_json::json!({});
    if let Some(change) = change {
        body["suggestion"] = change.to_json();
    }
    if let Some(note) = note {
        body["body"] = Value::String(note.to_owned());
    }
    Ok(body)
}

/// Edit a pending suggestion (author only). The anchor cannot change.
///
/// `POST /comments/{comment_id}/update/` with `suggestion{op, replacement}`
/// and/or the rationale as `body`.
pub async fn edit_suggestion(
    client: &ApiClient,
    comment_id: &str,
    change: Option<SuggestionChange<'_>>,
    note: Option<&str>,
) -> Result<Value, CliError> {
    let body = build_edit_suggestion_body(change, note)?;
    client
        .post_json(&comment_action_path(comment_id, "update"), &body)
        .await
}

/// Withdraw your own pending suggestion.
///
/// `POST /comments/{comment_id}/suggestion/withdraw/`. Sent without transport
/// replay: a lost reply is reported, never re-sent.
pub async fn withdraw_suggestion(client: &ApiClient, comment_id: &str) -> Result<Value, CliError> {
    client
        .post_json_no_replay(
            &comment_action_path(comment_id, "suggestion/withdraw"),
            &serde_json::json!({}),
        )
        .await
        .map_err(map_new_route_error)
}

/// Reject a pending suggestion. Metadata only; the note is not touched.
///
/// `POST /comments/{comment_id}/suggestion/reject/`. Sent without transport
/// replay.
pub async fn reject_suggestion(client: &ApiClient, comment_id: &str) -> Result<Value, CliError> {
    client
        .post_json_no_replay(
            &comment_action_path(comment_id, "suggestion/reject"),
            &serde_json::json!({}),
        )
        .await
        .map_err(map_new_route_error)
}

/// How long [`accept_suggestion`] waits before its single retry.
pub const ACCEPT_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(10);

/// Whether an accept refusal is one the server marks safe to repeat: `409
/// suggestion_needs_editor` with `retryable: true`, or `503
/// suggestion_apply_unknown` (a repeat accept returns the recorded outcome and
/// never applies twice). Nothing else is retried, including every transport
/// failure.
#[must_use]
pub fn accept_is_retryable(err: &CliError) -> bool {
    let (CliError::Api(api) | CliError::MappedApi { api, .. }) = err else {
        return false;
    };
    match (api.http_status, api.field_reason()) {
        (409, Some("suggestion_needs_editor")) => api.param_bool("retryable") == Some(true),
        (503, Some("suggestion_apply_unknown")) => true,
        _ => false,
    }
}

/// Accept a suggestion with the note closed (`mode=headless`), ONE attempt.
///
/// `POST /comments/{comment_id}/suggestion/accept/`, sent exactly once on the
/// wire (no replay on a gateway error, timeout, dropped connection or
/// redirect), because the accept rewrites the note. Every refusal is returned
/// as-is — this never retries. The MCP surface uses it directly.
pub async fn accept_suggestion_once(
    client: &ApiClient,
    comment_id: &str,
) -> Result<Value, CliError> {
    client
        .post_json_no_replay(
            &comment_action_path(comment_id, "suggestion/accept"),
            &serde_json::json!({ "mode": "headless" }),
        )
        .await
        .map_err(map_new_route_error)
}

/// Accept a suggestion with the note closed, as the CLI does: one
/// [`accept_suggestion_once`], and when the refusal is one the server marks
/// safe to repeat (see [`accept_is_retryable`]), the whole accept is repeated
/// ONCE after `retry_delay`; `on_retry` is told why first. Any other outcome
/// is returned as-is.
pub async fn accept_suggestion<F>(
    client: &ApiClient,
    comment_id: &str,
    retry_delay: std::time::Duration,
    on_retry: F,
) -> Result<Value, CliError>
where
    F: FnOnce(&CliError),
{
    match accept_suggestion_once(client, comment_id).await {
        Err(err) if accept_is_retryable(&err) => {
            on_retry(&err);
            tokio::time::sleep(retry_delay).await;
            accept_suggestion_once(client, comment_id).await
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AddCommentParams, AddSuggestionParams, AgentQualifier, Assignee, COMMENT_BODY_MAX_CHARS,
        CommentAttachTargets, CommentListFilters, QUOTE_CONTEXT_MAX, QUOTE_EXACT_MAX, QuoteAnchor,
        QuoteMatch, QuoteRequest, REFERENCE_MAX_BYTES, SuggestionChange, accept_is_retryable,
        apply_list_filters, build_add_comment_body, build_add_suggestion_body, build_assign_body,
        build_attach_body, build_detach_body, build_edit_suggestion_body, build_state_body,
        comment_action_path, map_new_route_error, resolve_quote_anchor,
        suggestion_create_was_downgraded, validate_comment_body,
    };
    use crate::error::{ApiError, CliError};
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn comment_body_max_is_characters_so_it_is_safe_in_both_windows() {
        assert!(validate_comment_body("hello").is_ok());
        assert!(validate_comment_body("").is_err());
        assert!(validate_comment_body("   ").is_err(), "blank after trim");

        // Boundary, in CHARACTERS.
        assert!(validate_comment_body(&"a".repeat(COMMENT_BODY_MAX_CHARS)).is_ok());
        assert!(validate_comment_body(&"a".repeat(COMMENT_BODY_MAX_CHARS + 1)).is_err());

        // The load-bearing property: a 3 000-character CJK body is 9 000 BYTES.
        // When this was written the byte server rejected it (162417) and the
        // client deliberately did not — the bound means characters, so the same
        // input is valid, and a byte mirror would have blocked it permanently.
        // Measured 2026-08-07: the server now counts characters too.
        //
        let cjk = "猫".repeat(3000);
        assert_eq!(cjk.chars().count(), 3000, "well under 8192 CHARACTERS");
        assert!(cjk.len() > 8192, "but well over 8192 BYTES");
        assert!(
            validate_comment_body(&cjk).is_ok(),
            "a character bound must stay lenient where the byte server is strict"
        );

        // And the converse direction can never happen: rejecting on characters
        // implies the byte count is at least as large, so anything this check
        // refuses today's byte server refuses too.
        let over = "猫".repeat(COMMENT_BODY_MAX_CHARS + 1);
        assert!(validate_comment_body(&over).is_err());
        assert!(over.len() > COMMENT_BODY_MAX_CHARS, "chars <= bytes always");
    }

    /// The over-length message names CHARACTERS and must not resurrect the
    /// retired byte-window caveat.
    ///
    /// This guard exists because the caveat outlived its window HERE by two
    /// days. It was removed from the `HINT_COMMENT_*` constants on 2026-08-07,
    /// the same hour the server was measured counting characters, and a test
    /// was written to keep it out — but that test's scope was the hints, and
    /// this string is a `CliError::Parse` built in `validate_comment_body`. The
    /// stale text sat one module away from a guard aimed at exactly it.
    ///
    /// So the negative half below is the point, not decoration: it is the check
    /// whose absence let a wrong sentence reach users for two days. Lowercase
    /// the haystack — the string that got away spelled it `BYTES`, and a
    /// case-sensitive needle would have missed it.
    #[test]
    fn over_length_message_states_characters_and_no_byte_caveat() {
        let over = "a".repeat(COMMENT_BODY_MAX_CHARS + 1);
        let Err(err) = validate_comment_body(&over) else {
            panic!("over-length body must be refused");
        };
        let msg = err.to_string();

        // Positive: the unit is named, and the actual count is reported so the
        // author can see how far over they are.
        assert!(
            msg.contains("characters"),
            "message must name the unit, got: {msg}"
        );
        assert!(
            msg.contains(&(COMMENT_BODY_MAX_CHARS + 1).to_string()),
            "message must report the measured length, got: {msg}"
        );

        // Negative: no byte vocabulary in any casing.
        assert!(
            !msg.to_lowercase().contains("byte"),
            "the byte-window caveat was retired 2026-08-07 — the server counts \
             characters and reports them; got: {msg}"
        );
    }

    #[test]
    fn add_comment_body_minimal_is_body_only() {
        let body = build_add_comment_body(&AddCommentParams {
            entity_type: "workspace",
            entity_id: "1",
            node_id: "n",
            body: "hi",
            parent_id: None,
            reference: None,
            properties: None,
            target_id: None,
            target_ids: None,
            reopen: false,
        });
        assert_eq!(body, json!({ "body": "hi" }));
    }

    #[test]
    fn add_comment_body_includes_reference_and_properties() {
        // `document` + a `page` anchor — the documented shape. This fixture
        // said `{"type":"page"}`, which the server REJECTS ("Invalid reference
        // type"). The test still passed because it asserts the body is
        // FORWARDED, not that the value is valid — so it could never have
        // caught the help text that told users the same wrong thing.
        let reference = json!({ "type": "document", "page": 3 });
        let properties = json!({ "k": "v" });
        let body = build_add_comment_body(&AddCommentParams {
            entity_type: "workspace",
            entity_id: "1",
            node_id: "n",
            body: "hi",
            parent_id: Some("p1"),
            reference: Some(&reference),
            properties: Some(&properties),
            target_id: None,
            target_ids: None,
            reopen: false,
        });
        assert_eq!(body["parent_id"], json!("p1"));
        assert_eq!(body["reference"], reference);
        assert_eq!(body["properties"], properties);
    }

    #[test]
    fn add_comment_body_inline_single_target() {
        let body = build_add_comment_body(&AddCommentParams {
            entity_type: "workspace",
            entity_id: "1",
            node_id: "n",
            body: "hi",
            parent_id: None,
            reference: None,
            properties: None,
            target_id: Some("t1"),
            target_ids: None,
            reopen: false,
        });
        assert_eq!(body["target_id"], json!("t1"));
        assert!(body.get("target_ids").is_none());
    }

    #[test]
    fn add_comment_body_inline_multiple_targets() {
        let ids = vec!["a".to_owned(), "b".to_owned()];
        let body = build_add_comment_body(&AddCommentParams {
            entity_type: "workspace",
            entity_id: "1",
            node_id: "n",
            body: "hi",
            parent_id: None,
            reference: None,
            properties: None,
            target_id: None,
            target_ids: Some(&ids),
            reopen: false,
        });
        assert_eq!(body["target_ids"], json!(["a", "b"]));
        assert!(body.get("target_id").is_none());
    }

    #[test]
    fn attach_body_single_uses_target_id() {
        let body = build_attach_body(&CommentAttachTargets::Single("t1"));
        assert_eq!(body, json!({ "target_id": "t1" }));
    }

    #[test]
    fn attach_body_multiple_uses_target_ids() {
        let ids = vec!["a".to_owned(), "b".to_owned()];
        let body = build_attach_body(&CommentAttachTargets::Multiple(&ids));
        assert_eq!(body, json!({ "target_ids": ["a", "b"] }));
    }

    #[test]
    fn detach_body_uses_target_id() {
        let body = build_detach_body("t1");
        assert_eq!(body, json!({ "target_id": "t1" }));
    }

    // ─── List filters ─────────────────────────────────────────────────────

    #[test]
    fn default_list_filters_add_nothing() {
        let mut query = HashMap::new();
        apply_list_filters(&mut query, &CommentListFilters::default());
        assert!(query.is_empty(), "no filter may be sent unless asked for");
    }

    #[test]
    fn list_filters_use_the_contract_names() {
        let mut query = HashMap::new();
        apply_list_filters(
            &mut query,
            &CommentListFilters {
                status: Some("open"),
                for_me: Some("agents"),
                kind: Some("suggestion"),
                include_suggestions: true,
                threads: true,
            },
        );
        assert_eq!(query.get("status").map(String::as_str), Some("open"));
        assert_eq!(query.get("for_me").map(String::as_str), Some("agents"));
        assert_eq!(query.get("kind").map(String::as_str), Some("suggestion"));
        assert_eq!(
            query.get("include_suggestions").map(String::as_str),
            Some("true")
        );
        assert_eq!(query.get("group").map(String::as_str), Some("threads"));
        assert_eq!(query.len(), 5);
    }

    // ─── Thread state ─────────────────────────────────────────────────────

    #[test]
    fn action_paths_are_dash_free() {
        assert_eq!(
            comment_action_path("cabcd-efghi-jklmn", "resolve"),
            "/comments/cabcdefghijklmn/resolve/"
        );
        assert_eq!(
            comment_action_path("c1", "suggestion/accept"),
            "/comments/c1/suggestion/accept/"
        );
    }

    #[test]
    fn state_body_sends_the_precondition_only_when_given() {
        assert_eq!(build_state_body(None), json!({}));
        assert_eq!(build_state_body(Some(7)), json!({ "if_state_version": 7 }));
    }

    #[test]
    fn assign_bodies_cover_every_form() {
        assert_eq!(
            build_assign_body(Some(&Assignee::Caller)),
            json!({ "assignee": "self" })
        );
        assert_eq!(
            build_assign_body(Some(&Assignee::Ripley)),
            json!({ "assignee": "ripley" })
        );
        assert_eq!(
            build_assign_body(Some(&Assignee::User {
                user_id: "1000000000000000002",
                agent: None,
            })),
            json!({ "assignee_user_id": "1000000000000000002" })
        );
        assert_eq!(
            build_assign_body(Some(&Assignee::User {
                user_id: "1000000000000000002",
                agent: Some(AgentQualifier {
                    agent_name: "helper",
                    credential_type: "oauth",
                    credential_id: Some("cred1"),
                }),
            })),
            json!({
                "assignee_user_id": "1000000000000000002",
                "assignee_agent": {
                    "agent_name": "helper",
                    "credential_type": "oauth",
                    "credential_id": "cred1"
                }
            })
        );
        assert_eq!(build_assign_body(None), json!({ "clear": true }));
    }

    #[test]
    fn reply_reopen_is_sent_only_when_asked() {
        let mut params = AddCommentParams {
            entity_type: "workspace",
            entity_id: "1",
            node_id: "n",
            body: "done",
            parent_id: Some("p1"),
            reference: None,
            properties: None,
            target_id: None,
            target_ids: None,
            reopen: false,
        };
        assert!(build_add_comment_body(&params).get("reopen").is_none());
        params.reopen = true;
        assert_eq!(build_add_comment_body(&params)["reopen"], json!(true));
    }

    #[test]
    fn a_reasonless_404_gets_the_not_supported_hint() {
        let mapped = map_new_route_error(CliError::Api(ApiError::new(
            9992,
            None,
            "not found".to_owned(),
            404,
        )));
        assert_eq!(
            mapped.suggestion(),
            Some(crate::error::HINT_COMMENT_ROUTE_NOT_FOUND)
        );

        // A 404 that names its reason keeps its own hint.
        let with_reason = map_new_route_error(CliError::Api(
            ApiError::new(1, None, "x".to_owned(), 404)
                .with_details(json!({ "params": { "reason": "comment_not_root" } })),
        ));
        assert!(matches!(with_reason, CliError::Api(_)));

        // Other statuses pass through untouched.
        let other = map_new_route_error(CliError::Api(ApiError::new(1, None, "x".to_owned(), 500)));
        assert!(matches!(other, CliError::Api(_)));
    }

    // ─── Quote anchors ────────────────────────────────────────────────────

    fn quote(exact: &str) -> QuoteRequest<'_> {
        QuoteRequest {
            exact,
            prefix: None,
            suffix: None,
            occurrence: None,
        }
    }

    fn resolved(text: &str, req: &QuoteRequest<'_>) -> (QuoteAnchor, QuoteMatch) {
        resolve_quote_anchor(Some(text), req).expect("anchor resolves")
    }

    #[test]
    fn unique_quote_derives_context_on_its_line() {
        let text = "First line.\nThe quick brown fox jumps over the lazy dog.\nLast line.";
        let (anchor, matched) = resolved(text, &quote("brown fox"));
        assert_eq!(matched, QuoteMatch::Unique);
        assert_eq!(anchor.prefix, "The quick ");
        assert_eq!(anchor.suffix, " jumps over the lazy dog.");
    }

    #[test]
    fn absent_quote_is_allowed_with_empty_context() {
        let (anchor, matched) = resolved("nothing here", &quote("missing words"));
        assert_eq!(matched, QuoteMatch::NotFound);
        assert_eq!(anchor.prefix, "");
        assert_eq!(anchor.suffix, "");
    }

    #[test]
    fn absent_quote_keeps_explicit_context() {
        let req = QuoteRequest {
            prefix: Some("before "),
            suffix: Some(" after"),
            ..quote("missing")
        };
        let (anchor, matched) = resolved("nothing here", &req);
        assert_eq!(matched, QuoteMatch::NotFound);
        assert_eq!(anchor.prefix, "before ");
        assert_eq!(anchor.suffix, " after");
    }

    #[test]
    fn repeated_quote_is_refused_without_an_occurrence() {
        let err = resolve_quote_anchor(Some("alpha one\nalpha two"), &quote("alpha"))
            .expect_err("two matches must be refused");
        let msg = err.to_string();
        assert!(msg.contains("2 times"), "{msg}");
        assert!(msg.contains("--occurrence"), "{msg}");
    }

    #[test]
    fn occurrence_picks_among_repeats_and_derives_its_context() {
        let text = "alpha one\nalpha two";
        let (first, _) = resolved(
            text,
            &QuoteRequest {
                occurrence: Some(1),
                ..quote("alpha")
            },
        );
        assert_eq!(first.suffix, " one");
        let (second, matched) = resolved(
            text,
            &QuoteRequest {
                occurrence: Some(2),
                ..quote("alpha")
            },
        );
        assert_eq!(matched, QuoteMatch::Unique);
        assert_eq!(second.suffix, " two");
    }

    #[test]
    fn occurrence_out_of_range_and_zero_are_refused() {
        let text = "alpha one\nalpha two";
        for k in [0, 3] {
            let req = QuoteRequest {
                occurrence: Some(k),
                ..quote("alpha")
            };
            assert!(resolve_quote_anchor(Some(text), &req).is_err(), "k={k}");
        }
        // Out of range even when the quote is absent.
        let req = QuoteRequest {
            occurrence: Some(1),
            ..quote("zzz")
        };
        assert!(resolve_quote_anchor(Some(text), &req).is_err());
    }

    #[test]
    fn occurrence_whose_context_is_still_ambiguous_is_refused() {
        // Both matches sit behind the same derivable context.
        let text = "- alpha\n- alpha";
        let req = QuoteRequest {
            occurrence: Some(1),
            ..quote("alpha")
        };
        let err = resolve_quote_anchor(Some(text), &req).expect_err("still ambiguous");
        assert!(err.to_string().contains("still occurs 2 times"), "{err}");
    }

    #[test]
    fn window_stops_at_bold_markers() {
        let (anchor, _) = resolved("see **the plan** below", &quote("the plan"));
        assert_eq!(anchor.prefix, "", "the rendered prefix is not `see **`");
        assert_eq!(anchor.suffix, "");

        let (anchor, _) = resolved("we use **bold** for the plan here", &quote("the plan"));
        assert_eq!(anchor.prefix, " for ");
        assert_eq!(anchor.suffix, " here");
    }

    #[test]
    fn window_stops_at_a_link() {
        // Walking back from after a link must not cross its hidden destination.
        let (anchor, _) = resolved("read [the docs](https://x.test) first", &quote("first"));
        assert_eq!(anchor.prefix, " ");
        let (anchor, _) = resolved("before [link text] after", &quote("link"));
        assert_eq!(anchor.prefix, "");
        assert_eq!(anchor.suffix, " text");
    }

    #[test]
    fn window_drops_line_leading_list_markers() {
        let (anchor, _) = resolved("- buy milk today", &quote("milk"));
        assert_eq!(anchor.prefix, "buy ");
        let (anchor, _) = resolved("12. buy milk today", &quote("milk"));
        assert_eq!(anchor.prefix, "buy ");
        let (anchor, _) = resolved("+ buy milk", &quote("milk"));
        assert_eq!(anchor.prefix, "buy ");
        // `-` with no following space is ordinary text.
        let (anchor, _) = resolved("-buy milk", &quote("milk"));
        assert_eq!(anchor.prefix, "-buy ");
        // Heading, blockquote and bullet markers are syntax, not text.
        let (anchor, _) = resolved("## Weekly milk report", &quote("milk"));
        assert_eq!(anchor.prefix, "Weekly ");
        let (anchor, _) = resolved("# Heading milk", &quote("milk"));
        assert_eq!(anchor.prefix, "Heading ");
        let (anchor, _) = resolved("> quoted milk", &quote("milk"));
        assert_eq!(anchor.prefix, "quoted ");
        let (anchor, _) = resolved("* buy milk", &quote("milk"));
        assert_eq!(anchor.prefix, "buy ");
        let (anchor, _) = resolved("1) buy milk", &quote("milk"));
        assert_eq!(anchor.prefix, "buy ");
    }

    #[test]
    fn window_drops_indented_nested_and_task_markers() {
        for line in [
            "  - buy milk",
            "\t- buy milk",
            "    - buy milk",
            "\t\t* buy milk",
            "> - buy milk",
            "> > buy milk",
            "- [ ] buy milk",
            "- [x] buy milk",
            "   3. buy milk",
        ] {
            let (anchor, _) = resolved(line, &quote("milk"));
            assert_eq!(anchor.prefix, "buy ", "line {line:?}");
        }
    }

    /// Line-start structure follows `CommonMark`: containers (`>` and list
    /// items) nest in any order, a heading is a leaf, and a task box counts
    /// only right after a list marker.
    #[test]
    fn line_lead_follows_commonmark_structure() {
        for (line, lead) in [
            ("1. 2. buy milk", "1. 2. "),
            ("- - buy milk", "- - "),
            ("- > buy milk", "- > "),
            ("> - buy milk", "> - "),
            ("- [ ] buy milk", "- [ ] "),
            ("# # Title milk", "# "),
            ("- # Title milk", "- # "),
            ("[ ] buy milk", ""),
            ("- [ ] [ ] buy milk", "- [ ] "),
            ("#hashtag milk", ""),
        ] {
            assert_eq!(
                super::line_lead_len(line),
                lead.len(),
                "line {line:?} should strip {lead:?}"
            );
        }
        // Through derivation. `# # Title`: the lead is `# `, and the window
        // then stops at the second `#` (a stop character) and drops the
        // heading padding beside it, giving `Title ` — still a suffix of the
        // rendered `# Title `.
        for (line, prefix) in [
            ("1. 2. buy milk", "buy "),
            ("- - buy milk", "buy "),
            ("- > buy milk", "buy "),
            ("> - buy milk", "buy "),
            ("- [ ] buy milk", "buy "),
            ("# # Title milk", "Title "),
            ("[ ] buy milk", " buy "),
        ] {
            let (anchor, _) = resolved(line, &quote("milk"));
            assert_eq!(anchor.prefix, prefix, "line {line:?}");
        }
    }

    #[test]
    fn window_keeps_text_that_only_looks_like_a_marker() {
        // Mid-line markers stop the window but are not line-start syntax.
        let (anchor, _) = resolved("we use **bold** for the plan", &quote("the plan"));
        assert_eq!(anchor.prefix, " for ");
        let (anchor, _) = resolved("#hashtag milk", &quote("milk"));
        assert_eq!(anchor.prefix, "hashtag ");
        let (anchor, _) = resolved("2024 was milk", &quote("milk"));
        assert_eq!(anchor.prefix, "2024 was ");
        let (anchor, _) = resolved("a  b milk", &quote("milk"));
        assert_eq!(anchor.prefix, "b ");
    }

    #[test]
    fn window_drops_table_and_heading_padding() {
        let (anchor, _) = resolved("| a milk |", &quote("milk"));
        assert_eq!(anchor.prefix, "a ");
        assert_eq!(anchor.suffix, "");
        let (anchor, _) = resolved("| x | a milk b | y |", &quote("milk"));
        assert_eq!(anchor.prefix, "a ");
        assert_eq!(anchor.suffix, " b");
        let (anchor, _) = resolved("## Title milk ##", &quote("milk"));
        assert_eq!(anchor.prefix, "Title ");
        assert_eq!(anchor.suffix, "");
    }

    #[test]
    fn window_stops_at_line_boundaries() {
        let (anchor, _) = resolved("one\r\ntwo three\r\nfour", &quote("two"));
        assert_eq!(anchor.prefix, "");
        assert_eq!(anchor.suffix, " three");
        // Trailing spaces at the end of the line do not render.
        let (anchor, _) = resolved("alpha beta \nnext", &quote("alpha"));
        assert_eq!(anchor.suffix, " beta");
    }

    #[test]
    fn window_stops_at_runs_of_spaces_and_tabs() {
        let (anchor, _) = resolved("a  b quote c\td", &quote("quote"));
        assert_eq!(anchor.prefix, "b ");
        assert_eq!(anchor.suffix, " c");
    }

    #[test]
    fn window_is_capped_at_the_context_limit() {
        let long = "x".repeat(250);
        let text = format!("{long} target {long}");
        let (anchor, _) = resolved(&text, &quote("target"));
        assert_eq!(anchor.prefix.encode_utf16().count(), QUOTE_CONTEXT_MAX);
        assert_eq!(anchor.suffix.encode_utf16().count(), QUOTE_CONTEXT_MAX);
    }

    #[test]
    fn multibyte_and_astral_text_is_measured_in_utf16() {
        let (anchor, _) = resolved("héllo wörld 猫 quote 🙂 tail", &quote("quote"));
        assert_eq!(anchor.prefix, "héllo wörld 猫 ");
        assert_eq!(anchor.suffix, " 🙂 tail");

        // 60 astral characters are 120 UTF-16 units: the window must stop at
        // 100 units (50 characters) and never split a surrogate pair.
        let astral = "🙂".repeat(60);
        let (anchor, _) = resolved(&format!("{astral}quote"), &quote("quote"));
        assert_eq!(anchor.prefix.chars().count(), 50);
        assert_eq!(anchor.prefix.encode_utf16().count(), QUOTE_CONTEXT_MAX);
    }

    #[test]
    fn exact_cap_is_500_utf16_units() {
        let ok = "a".repeat(QUOTE_EXACT_MAX);
        assert!(resolve_quote_anchor(Some(&ok), &quote(&ok)).is_ok());
        let over = "a".repeat(QUOTE_EXACT_MAX + 1);
        assert!(resolve_quote_anchor(Some(&over), &quote(&over)).is_err());
        // 250 astral characters are 500 units, at the cap; 251 are over.
        let both_empty = |exact: &str| {
            resolve_quote_anchor(
                None,
                &QuoteRequest {
                    prefix: Some(""),
                    suffix: Some(""),
                    ..quote(exact)
                },
            )
            .is_ok()
        };
        assert!(both_empty(&"🙂".repeat(250)));
        assert!(!both_empty(&"🙂".repeat(251)));
    }

    #[test]
    fn explicit_context_over_the_cap_is_refused() {
        let long = "p".repeat(QUOTE_CONTEXT_MAX + 1);
        let req = QuoteRequest {
            prefix: Some(&long),
            ..quote("x")
        };
        assert!(resolve_quote_anchor(Some("x"), &req).is_err());
    }

    #[test]
    fn reference_cap_is_2048_encoded_bytes() {
        let small = QuoteAnchor {
            exact: "quote".to_owned(),
            prefix: String::new(),
            suffix: String::new(),
        };
        let reference = small.to_reference().expect("small anchor fits");
        assert_eq!(
            reference,
            json!({ "type": "document", "exact": "quote", "prefix": "", "suffix": "" })
        );
        assert!(
            reference.get("start_offset").is_none(),
            "offsets are never sent"
        );

        // 500 CJK characters are 1500 bytes, plus 2 × 100 three-byte
        // characters of context: within every per-field cap, but over the
        // whole-object byte cap.
        let big = QuoteAnchor {
            exact: "猫".repeat(500),
            prefix: "猫".repeat(100),
            suffix: "猫".repeat(100),
        };
        let err = big.to_reference().expect_err("over the byte cap");
        assert!(err.to_string().contains(&REFERENCE_MAX_BYTES.to_string()));
    }

    #[test]
    fn explicit_context_is_still_checked_for_uniqueness() {
        let text = "the cat sat. the cat ran.";
        // An explicit prefix that does not disambiguate is refused.
        let req = QuoteRequest {
            prefix: Some("the "),
            ..quote("cat")
        };
        assert!(resolve_quote_anchor(Some(text), &req).is_err());
        // One that does is accepted, and the missing side is derived.
        let req = QuoteRequest {
            suffix: Some(" ran"),
            ..quote("cat")
        };
        let (anchor, matched) = resolved(text, &req);
        assert_eq!(matched, QuoteMatch::Unique);
        assert_eq!(anchor.prefix, "the cat sat. the ");
        assert_eq!(anchor.suffix, " ran");
        // Both explicit, and the pair matches twice: refused.
        let req = QuoteRequest {
            prefix: Some("the "),
            suffix: Some(" "),
            ..quote("cat")
        };
        assert!(resolve_quote_anchor(Some(text), &req).is_err());
    }

    #[test]
    fn unreadable_note_needs_both_explicit_sides() {
        assert!(resolve_quote_anchor(None, &quote("x")).is_err());
        let req = QuoteRequest {
            prefix: Some("a "),
            ..quote("x")
        };
        assert!(resolve_quote_anchor(None, &req).is_err());
        let req = QuoteRequest {
            prefix: Some("a "),
            suffix: Some(""),
            ..quote("x")
        };
        let (anchor, matched) = resolve_quote_anchor(None, &req).expect("explicit");
        assert_eq!(matched, QuoteMatch::Unchecked);
        assert_eq!(anchor.prefix, "a ");
    }

    #[test]
    fn uniqueness_counts_matches_hidden_by_inline_formatting() {
        // One raw match, but two once `**` is rendered away: refused.
        let text = "The **cat** sat.\n\nThe cat sat.";
        let req = QuoteRequest {
            prefix: Some("The "),
            suffix: Some(" sat."),
            ..quote("cat")
        };
        let err = resolve_quote_anchor(Some(text), &req).expect_err("ambiguous when rendered");
        assert!(err.to_string().contains("2 times"), "{err}");

        // Negative control: formatting elsewhere does not create a match.
        let text = "The cat sat.\n\nA **dog** sat.";
        let (_, matched) = resolved(text, &req);
        assert_eq!(matched, QuoteMatch::Unique);
    }

    #[test]
    fn link_destinations_and_hidden_markdown_are_not_candidates() {
        // The URL's "plan" is not visible: occurrence 2 is the SECOND visible
        // "plan", not the first.
        let text = "[spec](https://a.io/plan) - the plan; the plan.";
        let req = QuoteRequest {
            occurrence: Some(2),
            ..quote("plan")
        };
        let (anchor, matched) = resolved(text, &req);
        assert_eq!(matched, QuoteMatch::Unique);
        assert_eq!(anchor.prefix, " - the plan; the ");
        assert_eq!(anchor.suffix, ".");

        // Only inline link destinations are hidden. Reference definitions,
        // autolinks and HTML comments count as visible text, so a quote that
        // also appears there is refused (conservative), never mis-anchored.
        for text in [
            "[ref]: https://a.io/target\n\nsee target here",
            "<https://a.io/target> and see target here",
            "<!-- target --> see target here",
            "<!--target-->\ntarget",
        ] {
            let err = resolve_quote_anchor(Some(text), &quote("target"))
                .expect_err("over-counted, so refused");
            assert!(err.to_string().contains("2 times"), "{text:?}: {err}");
        }
    }

    #[test]
    fn autolink_text_is_visible_so_occurrence_one_is_the_first() {
        let text = "<https://a.io/target> x\n\nsee target y";
        let req = QuoteRequest {
            occurrence: Some(1),
            ..quote("target")
        };
        // Never the second ("see target y"): the first, or a refusal.
        if let Ok((anchor, _)) = resolve_quote_anchor(Some(text), &req) {
            assert!(!anchor.prefix.contains("see"), "{anchor:?}");
            assert_ne!(anchor.suffix, " y", "{anchor:?}");
        }
    }

    #[test]
    fn malformed_destinations_are_not_links() {
        // A space in the "destination": not a link, nothing is hidden.
        let text = "Use ](target one) here.\nUse target two.";
        let req = QuoteRequest {
            occurrence: Some(1),
            ..quote("target")
        };
        let (anchor, _) = resolved(text, &req);
        assert_eq!(anchor.suffix, " one", "the FIRST target: {anchor:?}");

        // An escaped `\)` does not close the destination, which hides the
        // first "target"; occurrence 2 is "B target two".
        let text = "[x](https://example.com/a\\)target)\nA target one.\nB target two.";
        let req = QuoteRequest {
            occurrence: Some(2),
            ..quote("target")
        };
        let (anchor, _) = resolved(text, &req);
        assert_eq!(anchor.prefix, "B ");
        assert_eq!(anchor.suffix, " two.");

        // Escaped and code-span brackets open no destination.
        assert!(super::inline_links("\\](x) and `](y)` and [a](b)").len() == 1);
        assert!(super::inline_links("[a](no close").is_empty());
    }

    #[test]
    fn occurrence_refuses_when_formatting_changes_the_count() {
        // Two visible matches in the markdown, but the rendered text (link
        // brackets gone) reads three: numbering would be unreliable.
        let text = "see ca[t](u) and cat and cat";
        let req = QuoteRequest {
            occurrence: Some(1),
            ..quote("cat")
        };
        let err = resolve_quote_anchor(Some(text), &req).expect_err("numbering unreliable");
        assert!(err.to_string().contains("occurrence numbering"), "{err}");
    }

    #[test]
    fn capped_prefix_never_starts_inside_an_entity() {
        let text = format!("A &amp;{}target", "x".repeat(99));
        let (anchor, _) = resolved(&text, &quote("target"));
        assert!(
            !anchor.prefix.starts_with(';') && !anchor.prefix.starts_with("amp;"),
            "{:?}",
            anchor.prefix
        );
        assert_eq!(anchor.prefix, "x".repeat(99));
    }

    #[test]
    fn quote_resolution_is_linear_on_large_notes() {
        let links = "[a](b) ".repeat(300_000 / 7);
        let started = std::time::Instant::now();
        let _ = resolve_quote_anchor(Some(&links), &quote("a"));
        let _ = resolve_quote_anchor(
            Some(&links),
            &QuoteRequest {
                occurrence: Some(1),
                ..quote("a")
            },
        );
        let elapsed = started.elapsed();
        eprintln!("quote resolution, 300 KB of links: {elapsed:?}");
        assert!(elapsed.as_millis() < 2_000, "links: {elapsed:?}");

        let ampersands = format!("{}{}", "&".repeat(300_000), "猫".repeat(1_000));
        let started = std::time::Instant::now();
        let _ = resolve_quote_anchor(Some(&ampersands), &quote("猫猫"));
        let _ = resolve_quote_anchor(Some(&ampersands), &quote("&猫"));
        let elapsed = started.elapsed();
        eprintln!("quote resolution, 300 KB of entities: {elapsed:?}");
        assert!(elapsed.as_millis() < 2_000, "entities: {elapsed:?}");
    }

    #[test]
    fn unclosed_destinations_are_bounded() {
        // One 300 KB line of `](` that never closes: each scan stops at the
        // destination cap instead of the end of the line.
        let text = "](".repeat(150_000);
        let started = std::time::Instant::now();
        let result = resolve_quote_anchor(Some(&text), &quote("x"));
        let elapsed = started.elapsed();
        eprintln!("quote resolution, 300 KB of unclosed `](`: {elapsed:?}");
        assert!(matches!(result, Ok((_, QuoteMatch::NotFound))));
        assert!(elapsed.as_millis() < 2_000, "unclosed: {elapsed:?}");
        assert!(super::inline_links(&text).is_empty());
    }

    #[test]
    fn window_stops_at_an_opening_parenthesis() {
        let (anchor, _) = resolved("see it (in the plan) now", &quote("plan"));
        assert_eq!(anchor.prefix, "in the ");
    }

    #[test]
    fn uniqueness_sees_through_links_escapes_and_entities() {
        let text = "The [cat](https://example.com) sat.\n\nThe cat sat.";
        let req = QuoteRequest {
            prefix: Some("The "),
            suffix: Some(" sat."),
            ..quote("cat")
        };
        assert!(resolve_quote_anchor(Some(text), &req).is_err(), "link text");

        let text = "Read [the plan](https://x) today.\nRead the plan today.";
        let req = QuoteRequest {
            occurrence: Some(2),
            ..quote("the plan")
        };
        assert!(resolve_quote_anchor(Some(text), &req).is_err(), "link text");

        for text in [
            "Tom &amp; Jerry ran.\n\nTom & Jerry ran.",
            "Tom &#38; Jerry ran.\n\nTom & Jerry ran.",
            "Tom \\& Jerry ran.\n\nTom & Jerry ran.",
        ] {
            let req = QuoteRequest {
                prefix: Some("Tom & "),
                suffix: Some(" ran."),
                ..quote("Jerry")
            };
            assert!(resolve_quote_anchor(Some(text), &req).is_err(), "{text:?}");
        }
        assert_eq!(
            super::approximate_rendered("a &lt;b&gt; &#x26; &quot;c&quot;"),
            "a <b> & \"c\""
        );
    }

    #[test]
    fn prefix_never_starts_inside_an_entity() {
        // The brief's fixture: whatever happens, never `amp; `.
        let text = "A &amp; target.\n\nB target.";
        let req = QuoteRequest {
            occurrence: Some(1),
            ..quote("target")
        };
        if let Ok((anchor, _)) = resolve_quote_anchor(Some(text), &req) {
            assert!(!anchor.prefix.contains("amp;"), "{anchor:?}");
        }
        for text in [
            "A &amp; target one.\n\nB target two.",
            "A &#38; target one.\n\nB target two.",
            "A &#x26; target one.\n\nB target two.",
        ] {
            let req = QuoteRequest {
                occurrence: Some(1),
                ..quote("target")
            };
            let (anchor, _) = resolved(text, &req);
            // Rendered "A & target": the prefix must be a suffix of "A & ".
            assert!(
                "A & ".ends_with(anchor.prefix.as_str()),
                "{text:?}: {anchor:?}"
            );
            assert_eq!(anchor.suffix, " one.", "{text:?}");
        }
    }

    // ─── Golden projection cases ──────────────────────────────────────────
    //
    // Markdown with its rendered plain text (one "\n" between text blocks,
    // marks and markers removed). Each derived anchor must be consistent with
    // the rendered text at the expected position, or be refused; a quote
    // expected to be ambiguous must be refused.

    /// What a golden anchor should resolve to.
    enum Expect {
        /// Exact, at this UTF-16 offset of the rendered text.
        At(usize),
        /// Ambiguous: must be refused.
        Ambiguous,
    }

    struct GoldenAnchor {
        exact: &'static str,
        prefix: Option<&'static str>,
        suffix: Option<&'static str>,
        occurrence: Option<usize>,
        expect: Expect,
    }

    struct GoldenCase {
        name: &'static str,
        markdown: &'static str,
        rendered: &'static str,
        anchors: &'static [GoldenAnchor],
    }

    const GOLDEN: &[GoldenCase] = &[
        GoldenCase {
            name: "inline-marks",
            markdown: "see **the plan** below\n\nSecond paragraph with *emphasis*, `code` and a [link](https://example.com/x).\n",
            rendered: "see the plan below\nSecond paragraph with emphasis, code and a link.",
            anchors: &[
                GoldenAnchor {
                    exact: "the plan",
                    prefix: None,
                    suffix: None,
                    occurrence: None,
                    expect: Expect::At(4),
                },
                GoldenAnchor {
                    exact: "emphasis, code and a link",
                    prefix: None,
                    suffix: None,
                    occurrence: None,
                    expect: Expect::At(41),
                },
                GoldenAnchor {
                    exact: "link",
                    prefix: None,
                    suffix: None,
                    occurrence: None,
                    expect: Expect::At(62),
                },
            ],
        },
        GoldenCase {
            name: "duplicates",
            markdown: "The cat sat. The cat ran.\n\nThe cat slept.\n",
            rendered: "The cat sat. The cat ran.\nThe cat slept.",
            anchors: &[
                GoldenAnchor {
                    exact: "cat",
                    prefix: None,
                    suffix: None,
                    occurrence: Some(2),
                    expect: Expect::At(17),
                },
                GoldenAnchor {
                    exact: "cat",
                    prefix: None,
                    suffix: None,
                    occurrence: Some(3),
                    expect: Expect::At(30),
                },
                GoldenAnchor {
                    exact: "cat",
                    prefix: Some(""),
                    suffix: Some(""),
                    occurrence: None,
                    expect: Expect::Ambiguous,
                },
                GoldenAnchor {
                    exact: "cat",
                    prefix: Some("The "),
                    suffix: Some(" "),
                    occurrence: None,
                    expect: Expect::Ambiguous,
                },
                GoldenAnchor {
                    exact: "cat",
                    prefix: Some(""),
                    suffix: Some(" ran."),
                    occurrence: None,
                    expect: Expect::At(17),
                },
                GoldenAnchor {
                    exact: "slept",
                    prefix: Some("zzz"),
                    suffix: Some("zzz"),
                    occurrence: None,
                    expect: Expect::At(34),
                },
            ],
        },
        GoldenCase {
            name: "overlapping-occurrences",
            markdown: "aaaa\n",
            rendered: "aaaa",
            anchors: &[
                GoldenAnchor {
                    exact: "aa",
                    prefix: Some(""),
                    suffix: Some(""),
                    occurrence: None,
                    expect: Expect::Ambiguous,
                },
                GoldenAnchor {
                    exact: "aa",
                    prefix: Some("a"),
                    suffix: Some("a"),
                    occurrence: None,
                    expect: Expect::At(1),
                },
            ],
        },
        GoldenCase {
            name: "emoji-surrogates",
            markdown: "Hello \u{1f44b} world \u{1f389}\u{1f389} done\n\nFamily \u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} and cafe\u{301} and caf\u{e9}.\n",
            rendered: "Hello \u{1f44b} world \u{1f389}\u{1f389} done\nFamily \u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} and cafe\u{301} and caf\u{e9}.",
            anchors: &[
                GoldenAnchor {
                    exact: "\u{1f389}",
                    prefix: None,
                    suffix: None,
                    occurrence: Some(2),
                    expect: Expect::At(17),
                },
                GoldenAnchor {
                    exact: "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}",
                    prefix: None,
                    suffix: None,
                    occurrence: None,
                    expect: Expect::At(32),
                },
                GoldenAnchor {
                    exact: "cafe\u{301}",
                    prefix: None,
                    suffix: None,
                    occurrence: None,
                    expect: Expect::At(45),
                },
                GoldenAnchor {
                    exact: "caf\u{e9}",
                    prefix: None,
                    suffix: None,
                    occurrence: None,
                    expect: Expect::At(55),
                },
                GoldenAnchor {
                    exact: "\u{1f44b}",
                    prefix: Some("Hello "),
                    suffix: Some(" world"),
                    occurrence: None,
                    expect: Expect::At(6),
                },
            ],
        },
        GoldenCase {
            name: "nbsp",
            markdown: "Price:\u{a0}100\u{a0}USD here\n\n\u{a0}\n\nAfter the marker line\n",
            rendered: "Price:\u{a0}100\u{a0}USD here\n\nAfter the marker line",
            anchors: &[GoldenAnchor {
                exact: "100\u{a0}USD",
                prefix: None,
                suffix: None,
                occurrence: None,
                expect: Expect::At(7),
            }],
        },
        GoldenCase {
            name: "hard-breaks",
            markdown: "Line one\\\nLine two\\\nLine three\n\nNext paragraph\n",
            rendered: "Line oneLine twoLine three\nNext paragraph",
            anchors: &[
                GoldenAnchor {
                    exact: "oneLine two",
                    prefix: None,
                    suffix: None,
                    occurrence: None,
                    expect: Expect::At(5),
                },
                GoldenAnchor {
                    exact: "three\nNext",
                    prefix: None,
                    suffix: None,
                    occurrence: None,
                    expect: Expect::At(21),
                },
                GoldenAnchor {
                    exact: "Line two",
                    prefix: None,
                    suffix: None,
                    occurrence: None,
                    expect: Expect::At(8),
                },
            ],
        },
        GoldenCase {
            name: "empty-blocks",
            markdown: "First\n\n\u{a0}\n\n\u{a0}\n\nAfter two empty\n",
            rendered: "First\n\n\nAfter two empty",
            anchors: &[GoldenAnchor {
                exact: "First\n\n\nAfter",
                prefix: None,
                suffix: None,
                occurrence: None,
                expect: Expect::At(0),
            }],
        },
        GoldenCase {
            name: "table",
            markdown: "| A          | B          |\n| ---------- | ---------- |\n| one        | two        |\n| three      | four       |\n\nAfter table\n",
            rendered: "A\nB\none\ntwo\nthree\nfour\nAfter table",
            anchors: &[
                GoldenAnchor {
                    exact: "two",
                    prefix: None,
                    suffix: None,
                    occurrence: None,
                    expect: Expect::At(8),
                },
                GoldenAnchor {
                    exact: "two\nthree",
                    prefix: None,
                    suffix: None,
                    occurrence: None,
                    expect: Expect::At(8),
                },
            ],
        },
        GoldenCase {
            name: "lists",
            markdown: "* alpha\n  * nested beta\n* gamma\n\n1. first\n2. second\n\n* [ ] todo item\n* [x] done item\n",
            rendered: "alpha\nnested beta\ngamma\nfirst\nsecond\ntodo item\ndone item",
            anchors: &[
                GoldenAnchor {
                    exact: "nested beta",
                    prefix: None,
                    suffix: None,
                    occurrence: None,
                    expect: Expect::At(6),
                },
                GoldenAnchor {
                    exact: "beta\ngamma",
                    prefix: None,
                    suffix: None,
                    occurrence: None,
                    expect: Expect::At(13),
                },
                GoldenAnchor {
                    exact: "done item",
                    prefix: None,
                    suffix: None,
                    occurrence: None,
                    expect: Expect::At(47),
                },
            ],
        },
        GoldenCase {
            name: "code",
            markdown: "Before\n\n```js\nconst a = 1;\nconst b = 2;\n```\n\nInline `x` code\n",
            rendered: "Before\nconst a = 1;\nconst b = 2;\nInline x code",
            anchors: &[
                GoldenAnchor {
                    exact: "1;\nconst b",
                    prefix: None,
                    suffix: None,
                    occurrence: None,
                    expect: Expect::At(17),
                },
                GoldenAnchor {
                    exact: "x code",
                    prefix: None,
                    suffix: None,
                    occurrence: None,
                    expect: Expect::At(40),
                },
            ],
        },
        GoldenCase {
            name: "cross-block",
            markdown: "# Heading\n\nParagraph one ends here.\n\n## Sub\n\nParagraph two starts here.\n",
            rendered: "Heading\nParagraph one ends here.\nSub\nParagraph two starts here.",
            anchors: &[
                GoldenAnchor {
                    exact: "Heading\nParagraph one",
                    prefix: None,
                    suffix: None,
                    occurrence: None,
                    expect: Expect::At(0),
                },
                GoldenAnchor {
                    exact: "here.\nSub\nParagraph two",
                    prefix: None,
                    suffix: None,
                    occurrence: None,
                    expect: Expect::At(27),
                },
                GoldenAnchor {
                    exact: "Heading",
                    prefix: None,
                    suffix: None,
                    occurrence: None,
                    expect: Expect::At(0),
                },
            ],
        },
        GoldenCase {
            name: "media-html",
            markdown: "Before media\n\n![clip.mp4](https://example.com/clip.mp4)\n\nBetween\n\n<audio src=\"https://example.com/sound.mp3\" controls></audio>\n\n[doc.pdf](https://example.com/doc.pdf)\n\nAfter media\n",
            rendered: "Before media\nBetween\ndoc.pdf\nAfter media",
            anchors: &[
                GoldenAnchor {
                    exact: "media\nBetween",
                    prefix: None,
                    suffix: None,
                    occurrence: None,
                    expect: Expect::At(7),
                },
                GoldenAnchor {
                    exact: "doc.pdf\nAfter",
                    prefix: None,
                    suffix: None,
                    occurrence: None,
                    expect: Expect::At(21),
                },
                GoldenAnchor {
                    exact: "doc.pdf",
                    prefix: None,
                    suffix: None,
                    occurrence: None,
                    expect: Expect::At(21),
                },
            ],
        },
    ];

    /// Byte index of a UTF-16 offset.
    fn byte_at_utf16(s: &str, units: usize) -> usize {
        let mut seen = 0;
        for (i, c) in s.char_indices() {
            if seen >= units {
                return i;
            }
            seen += c.len_utf16();
        }
        s.len()
    }

    #[test]
    fn golden_projection_cases() {
        let mut refused = Vec::new();
        for case in GOLDEN {
            for a in case.anchors {
                let req = QuoteRequest {
                    exact: a.exact,
                    prefix: a.prefix,
                    suffix: a.suffix,
                    occurrence: a.occurrence,
                };
                let result = resolve_quote_anchor(Some(case.markdown), &req);
                let needle = format!(
                    "{}{}{}",
                    a.prefix.unwrap_or_default(),
                    a.exact,
                    a.suffix.unwrap_or_default()
                );
                let in_markdown = case.markdown.contains(&needle);
                match (&a.expect, result) {
                    (Expect::Ambiguous, outcome) => assert!(
                        outcome.is_err(),
                        "{}: {:?} must be refused as ambiguous",
                        case.name,
                        a.exact
                    ),
                    (Expect::At(offset), Ok((anchor, QuoteMatch::Unique))) => {
                        let start = byte_at_utf16(case.rendered, *offset);
                        let end = start + a.exact.len();
                        assert_eq!(
                            case.rendered.get(start..end),
                            Some(a.exact),
                            "{}",
                            case.name
                        );
                        assert!(
                            case.rendered[..start].ends_with(&anchor.prefix),
                            "{}: prefix {:?} is not rendered text before {:?}",
                            case.name,
                            anchor.prefix,
                            a.exact
                        );
                        assert!(
                            case.rendered[end..].starts_with(&anchor.suffix),
                            "{}: suffix {:?} is not rendered text after {:?}",
                            case.name,
                            anchor.suffix,
                            a.exact
                        );
                    }
                    (Expect::At(_), Ok((anchor, matched))) => {
                        // Only a quote that is NOT in the stored markdown as
                        // written (it spans blocks or marks) may come back
                        // unmatched, sent with only the given context.
                        assert!(
                            !in_markdown && matched == QuoteMatch::NotFound,
                            "{}: {:?} is in the markdown, so it must resolve uniquely",
                            case.name,
                            a.exact
                        );
                        assert_eq!(anchor.prefix, a.prefix.unwrap_or_default(), "{}", case.name);
                        assert_eq!(anchor.suffix, a.suffix.unwrap_or_default(), "{}", case.name);
                    }
                    (Expect::At(_), Err(_)) => {
                        refused.push(format!("{}: {:?}", case.name, a.exact));
                    }
                }
            }
        }
        // Conservative refusals are allowed but pinned, so a change shows up.
        assert!(refused.is_empty(), "unexpected refusals: {refused:?}");
    }

    #[test]
    fn blank_quote_is_refused() {
        assert!(resolve_quote_anchor(Some("text"), &quote("   ")).is_err());
    }

    // ─── Suggestions ──────────────────────────────────────────────────────

    #[test]
    fn suggestion_body_carries_rationale_in_body() {
        let reference = json!({ "type": "document", "exact": "x", "prefix": "", "suffix": "" });
        let body = build_add_suggestion_body(&AddSuggestionParams {
            entity_type: "workspace",
            entity_id: "1",
            node_id: "n",
            reference: &reference,
            change: SuggestionChange::Replace("y"),
            note: Some("why"),
        });
        assert_eq!(
            body,
            json!({
                "body": "why",
                "kind": "suggestion",
                "reference": reference,
                "suggestion": { "op": "replace", "replacement": "y" }
            })
        );

        let body = build_add_suggestion_body(&AddSuggestionParams {
            entity_type: "workspace",
            entity_id: "1",
            node_id: "n",
            reference: &reference,
            change: SuggestionChange::Delete,
            note: None,
        });
        assert_eq!(body["body"], json!(""), "an empty rationale is allowed");
        assert_eq!(
            body["suggestion"],
            json!({ "op": "delete", "replacement": "" })
        );
    }

    #[test]
    fn replacement_is_sent_verbatim() {
        let body = SuggestionChange::Replace("  padded  ").to_json();
        assert_eq!(body["replacement"], json!("  padded  "));
    }

    #[test]
    fn edit_body_requires_a_change() {
        assert!(build_edit_suggestion_body(None, None).is_err());
        assert_eq!(
            build_edit_suggestion_body(Some(SuggestionChange::Replace("z")), None).expect("body"),
            json!({ "suggestion": { "op": "replace", "replacement": "z" } })
        );
        assert_eq!(
            build_edit_suggestion_body(None, Some("new reason")).expect("body"),
            json!({ "body": "new reason" })
        );
    }

    #[test]
    fn downgrade_detection_reads_the_returned_kind() {
        assert!(!suggestion_create_was_downgraded(
            &json!({ "comment": { "kind": "suggestion" } })
        ));
        assert!(suggestion_create_was_downgraded(
            &json!({ "comment": { "id": "c1" } })
        ));
        assert!(!suggestion_create_was_downgraded(&json!({ "other": 1 })));
    }

    fn api_err(status: u16, params: &serde_json::Value) -> CliError {
        CliError::Api(
            ApiError::new(1, None, "refused".to_owned(), status)
                .with_details(json!({ "params": params })),
        )
    }

    #[test]
    fn only_server_marked_refusals_are_retryable() {
        assert!(accept_is_retryable(&api_err(
            409,
            &json!({ "reason": "suggestion_needs_editor", "retryable": true, "editor_reason": "room_busy" })
        )));
        assert!(accept_is_retryable(&api_err(
            503,
            &json!({ "reason": "suggestion_apply_unknown" })
        )));
        for (status, params) in [
            (
                409,
                json!({ "reason": "suggestion_needs_editor", "retryable": false }),
            ),
            (409, json!({ "reason": "suggestion_needs_editor" })),
            (409, json!({ "reason": "suggestion_stale" })),
            (409, json!({ "reason": "suggestion_not_pending" })),
            (
                503,
                json!({ "reason": "suggestion_apply_unavailable", "retryable": true }),
            ),
            (403, json!({ "reason": "suggestion_accept_forbidden" })),
            (503, json!({})),
        ] {
            assert!(
                !accept_is_retryable(&api_err(status, &params)),
                "{status} {params} must not be retried"
            );
        }
        assert!(!accept_is_retryable(&CliError::Parse("x".to_owned())));
    }
}
