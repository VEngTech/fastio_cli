#![allow(clippy::missing_errors_doc)]

//! Note export endpoints: start a server-side export job, read its status,
//! and download the finished file.
//!
//! An export is an asynchronous job. `POST …/storage/{node}/export/` answers
//! `202` with a `queued` record (or `200` with an already `succeeded` one for
//! `md`), `GET /exports/{id}/` reports progress, and `GET
//! /exports/{id}/download/` streams the file for a download-destination export.
//! The job's outcome is always `export.output`; the top-level `result` is only
//! the API's success flag.

use std::future::Future;
use std::time::Duration;

use serde_json::Value;

use crate::client::ApiClient;
use crate::error::{ApiError, CliError};

/// Export formats the server renders.
pub const EXPORT_FORMATS: &[&str] = &["pdf", "docx", "odt", "html", "txt", "md"];

/// How comment threads appear in the export.
pub const EXPORT_COMMENT_MODES: &[&str] = &["none", "margin", "native", "endnotes"];

/// Page sizes for paged formats (pdf, docx, odt).
pub const EXPORT_PAGE_SIZES: &[&str] = &["letter", "a4"];

/// Page margins for paged formats (pdf, docx, odt).
pub const EXPORT_MARGINS: &[&str] = &["narrow", "normal", "wide"];

/// Formats that can carry pending suggestions as native tracked changes.
pub const TRACKED_CHANGES_FORMATS: &[&str] = &["docx", "odt"];

/// Default comment mode: no comments.
pub const DEFAULT_COMMENT_MODE: &str = "none";

/// Default page size.
pub const DEFAULT_PAGE_SIZE: &str = "letter";

/// Default margins.
pub const DEFAULT_MARGINS: &str = "normal";

/// Where the finished export goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExportDestination<'a> {
    /// Kept server-side for download through `GET /exports/{id}/download/`.
    Download,
    /// Saved in the workspace, in `parent_id` or, when `None`, next to the
    /// note: a new file, except that an `md` export becomes a new note.
    Workspace {
        /// Target folder node id.
        parent_id: Option<&'a str>,
    },
}

/// Parameters for [`start_export`].
#[derive(Debug, Clone, Copy)]
pub struct StartExportParams<'a> {
    /// Workspace id.
    pub workspace_id: &'a str,
    /// Node id of the note.
    pub node_id: &'a str,
    /// One of [`EXPORT_FORMATS`].
    pub format: &'a str,
    /// Export a specific note version instead of the current one.
    pub version_id: Option<&'a str>,
    /// File name for the exported file.
    pub file_name: Option<&'a str>,
    /// One of [`EXPORT_PAGE_SIZES`].
    pub page_size: &'a str,
    /// One of [`EXPORT_MARGINS`].
    pub margins: &'a str,
    /// One of [`EXPORT_COMMENT_MODES`].
    pub comments: &'a str,
    /// Include resolved comment threads.
    pub include_resolved: bool,
    /// Export pending suggestions as native tracked changes (docx/odt only).
    pub tracked_changes: bool,
    /// Where the finished file goes.
    pub destination: ExportDestination<'a>,
    /// One key per invocation, reused on every transport replay of the start
    /// request, so a replay returns the same export instead of a second one.
    pub idempotency_key: &'a str,
}

/// Check the parameters the client can judge on its own: the enumerated
/// values, and tracked changes on a format that cannot carry them.
pub fn validate_start(params: &StartExportParams<'_>) -> Result<(), CliError> {
    let check = |name: &str, value: &str, allowed: &[&str]| {
        if allowed.contains(&value) {
            Ok(())
        } else {
            Err(CliError::Parse(format!(
                "invalid {name} '{value}' (valid: {})",
                allowed.join(", ")
            )))
        }
    };
    check("format", params.format, EXPORT_FORMATS)?;
    check("comments mode", params.comments, EXPORT_COMMENT_MODES)?;
    check("page size", params.page_size, EXPORT_PAGE_SIZES)?;
    check("margins", params.margins, EXPORT_MARGINS)?;
    if params.tracked_changes && !TRACKED_CHANGES_FORMATS.contains(&params.format) {
        return Err(CliError::Parse(format!(
            "tracked changes need docx or odt, not {}",
            params.format
        )));
    }
    if params.idempotency_key.trim().is_empty() {
        return Err(CliError::Parse(
            "an export needs an idempotency key".to_owned(),
        ));
    }
    Ok(())
}

/// The JSON body of the start request.
#[must_use]
pub fn start_body(params: &StartExportParams<'_>) -> Value {
    let mut map = serde_json::Map::new();
    let mut put = |key: &str, value: Value| {
        map.insert(key.to_owned(), value);
    };
    put("format", params.format.into());
    put("page_size", params.page_size.into());
    put("margins", params.margins.into());
    put("comments", params.comments.into());
    put("include_resolved", params.include_resolved.into());
    let suggestions = if params.tracked_changes {
        "native"
    } else {
        "none"
    };
    put("suggestions", suggestions.into());
    put("idempotency_key", params.idempotency_key.into());
    if let Some(version_id) = params.version_id {
        put("version_id", version_id.into());
    }
    if let Some(file_name) = params.file_name {
        put("file_name", file_name.into());
    }
    match params.destination {
        ExportDestination::Download => put("destination", "download".into()),
        ExportDestination::Workspace { parent_id } => {
            put("destination", "workspace".into());
            if let Some(parent_id) = parent_id {
                put("parent_id", parent_id.into());
            }
        }
    }
    Value::Object(map)
}

/// Mint the per-invocation idempotency key: a random (version 4) UUID.
pub fn new_idempotency_key() -> Result<String, CliError> {
    let mut b = [0u8; 16];
    getrandom_crate::fill(&mut b)
        .map_err(|e| CliError::Config(format!("failed to generate random bytes: {e}")))?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().fold(String::with_capacity(32), |mut acc, byte| {
        use std::fmt::Write as _;
        let _ = write!(acc, "{byte:02x}");
        acc
    });
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

/// Re-hint a reasonless `404` from an export route: an unknown route and an
/// unknown id look the same from here.
fn map_export_route_error(err: CliError) -> CliError {
    match err {
        CliError::Api(api) if api.http_status == 404 && api.field_reason().is_none() => {
            CliError::MappedApi {
                api,
                hint: Some(crate::error::HINT_EXPORT_NOT_FOUND),
            }
        }
        other => other,
    }
}

/// `/exports/{id}/` plus `suffix`. The export id is opaque and passed through
/// unchanged apart from trimming and encoding.
fn export_path(export_id: &str, suffix: &str) -> String {
    format!(
        "/exports/{}/{suffix}",
        urlencoding::encode(export_id.trim())
    )
}

/// Start an export job.
///
/// `POST /workspace/{ws}/storage/{node}/export/` with a JSON body. Sent with
/// the ordinary transport replay: the idempotency key makes a replayed start
/// return the same export rather than a second one. Returns the unwrapped
/// payload; read the job with [`export_record`].
pub async fn start_export(
    client: &ApiClient,
    params: &StartExportParams<'_>,
) -> Result<Value, CliError> {
    validate_start(params)?;
    let path = format!(
        "/workspace/{}/storage/{}/export/",
        urlencoding::encode(params.workspace_id.trim()),
        urlencoding::encode(&crate::opaque_id::canonicalize(params.node_id)),
    );
    client
        .post_json(&path, &start_body(params))
        .await
        .map_err(map_export_route_error)
}

/// Read an export job's status. `GET /exports/{id}/`.
pub async fn export_status(client: &ApiClient, export_id: &str) -> Result<Value, CliError> {
    client
        .get(&export_path(export_id, ""))
        .await
        .map_err(map_export_route_error)
}

/// Download a finished export to `output_path`.
///
/// `GET /exports/{id}/download/` with the bearer token, on a client that never
/// follows a redirect and refuses a JSON body in place of the file. A failed
/// download leaves no partial file and keeps any file already at
/// `output_path`.
pub async fn download_export(
    client: &ApiClient,
    export_id: &str,
    output_path: &std::path::Path,
) -> Result<u64, CliError> {
    client
        .download_file_stream_strict(&export_path(export_id, "download/"), output_path)
        .await
        .map_err(map_export_route_error)
}

/// The export record inside a start or status payload: `export`, or
/// `response.export` when the envelope was not unwrapped.
#[must_use]
pub fn export_record(payload: &Value) -> Option<&Value> {
    payload
        .get("export")
        .or_else(|| payload.get("response").and_then(|r| r.get("export")))
        .filter(|v| v.is_object())
}

/// The lifecycle state of an export record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExportState {
    /// Waiting for a worker.
    Queued,
    /// Rendering.
    Running,
    /// Finished; `output` says where the file is.
    Succeeded,
    /// Finished without a file; `error` says why.
    Failed,
    /// A state this client does not know. Treated as not finished.
    Unknown,
}

impl ExportState {
    /// Whether the job has finished, successfully or not.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed)
    }
}

/// Read the state of an export record.
#[must_use]
pub fn export_state(record: &Value) -> ExportState {
    match record.get("status").and_then(Value::as_str) {
        Some("queued") => ExportState::Queued,
        Some("running") => ExportState::Running,
        Some("succeeded") => ExportState::Succeeded,
        Some("failed") => ExportState::Failed,
        _ => ExportState::Unknown,
    }
}

/// The export id of a record, as a string (a numeric id is rendered).
#[must_use]
pub fn export_id(record: &Value) -> Option<String> {
    match record.get("id")? {
        Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The workspace node a saved-copy export created (`output.node_id`).
#[must_use]
pub fn saved_node_id(record: &Value) -> Option<&str> {
    record
        .get("output")
        .and_then(|o| o.get("node_id"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

/// A failed record's error as an [`ApiError`], so the shared reason-hint table
/// applies. `None` unless the record is `failed`.
///
/// The job's `error` (`reason`, `retryable`, `retry_after`, and any further
/// keys such as `comments_droppable`) becomes the error's `params`.
#[must_use]
pub fn failure_error(record: &Value) -> Option<ApiError> {
    if export_state(record) != ExportState::Failed {
        return None;
    }
    let error = record.get("error").filter(|e| e.is_object());
    let mut params = error
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    // Some renderings nest the extras under `error.params`; flatten them so
    // `comments_droppable` is found either way.
    if let Some(Value::Object(nested)) = params.remove("params") {
        for (k, v) in nested {
            params.entry(k).or_insert(v);
        }
    }
    let reason = params
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let retryable = params.get("retryable").and_then(Value::as_bool) == Some(true);
    let message = format!(
        "the export job failed: {reason}{}",
        if retryable { " (retryable)" } else { "" }
    );
    Some(
        ApiError::new(0, None, message, 200)
            .with_details(serde_json::json!({ "params": Value::Object(params) })),
    )
}

/// How the status poll paces itself.
#[derive(Debug, Clone, Copy)]
pub struct PollPolicy {
    /// Delay before the first status read after a non-terminal record.
    pub initial_delay: Duration,
    /// Cap for the doubling delay.
    pub max_delay: Duration,
    /// Upper bound on one status request.
    pub request_timeout: Duration,
    /// The least time a read is given: no read starts with less of the
    /// deadline left than this.
    pub min_read_window: Duration,
}

impl PollPolicy {
    /// 1 s doubling to 10 s, each status request bounded at 30 s, and no
    /// read started with under 2 s left.
    pub const DEFAULT: Self = Self {
        initial_delay: Duration::from_secs(1),
        max_delay: Duration::from_secs(10),
        request_timeout: Duration::from_secs(30),
        min_read_window: Duration::from_secs(2),
    };
}

/// How a poll ended.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum PollOutcome {
    /// The job reached `succeeded` or `failed`; the final record.
    Finished(Value),
    /// The deadline passed first; the last record seen, if any.
    DeadlineReached(Option<Value>),
}

/// Poll an export until it finishes or `deadline` passes.
///
/// `initial` is a record already in hand (the start response); when it is
/// terminal no request is made. Otherwise `fetch` is called for the status
/// payload, with a delay of `policy.initial_delay` doubling to
/// `policy.max_delay` between reads, and never past the deadline. When no
/// record is in hand, the first read happens immediately and always runs, so a
/// zero deadline is a single check.
///
/// Every read is bounded by the smaller of `policy.request_timeout` and the
/// time left, except that single check under a zero deadline, which gets the
/// full `policy.request_timeout`. A read that times out, or a payload without
/// a record, counts as "still running". A read that fails is returned as the
/// error. Nothing is ever re-submitted: this only reads. A deadline too large
/// to represent is an error, not a panic.
pub async fn poll_export<F, Fut>(
    initial: Option<Value>,
    mut fetch: F,
    deadline: Duration,
    policy: PollPolicy,
) -> Result<PollOutcome, CliError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Value, CliError>>,
{
    let end = tokio::time::Instant::now()
        .checked_add(deadline)
        .ok_or_else(|| CliError::Parse(format!("timeout too large: {}s", deadline.as_secs())))?;
    let mut delay = policy.initial_delay;
    let mut forced_read = initial.is_none() && deadline.is_zero();
    let mut last = initial;

    if let Some(record) = &last {
        if export_state(record).is_terminal() {
            return Ok(PollOutcome::Finished(record.clone()));
        }
        if !sleep_before_next(&mut delay, policy, end).await {
            return Ok(PollOutcome::DeadlineReached(last));
        }
    }

    loop {
        let bound = if forced_read {
            policy.request_timeout
        } else {
            policy
                .request_timeout
                .min(end.saturating_duration_since(tokio::time::Instant::now()))
        };
        forced_read = false;
        if let Ok(result) = tokio::time::timeout(bound, fetch()).await {
            let payload = result?;
            if let Some(record) = export_record(&payload) {
                if export_state(record).is_terminal() {
                    return Ok(PollOutcome::Finished(record.clone()));
                }
                last = Some(record.clone());
            }
        }
        if !sleep_before_next(&mut delay, policy, end).await {
            return Ok(PollOutcome::DeadlineReached(last));
        }
    }
}

/// Sleep for the current delay, shortened so at least
/// `policy.min_read_window` of the deadline is left for the next read, then
/// double the delay up to the cap. `false` when no such window is left, so the
/// caller stops instead of reading once more.
async fn sleep_before_next(
    delay: &mut Duration,
    policy: PollPolicy,
    end: tokio::time::Instant,
) -> bool {
    let left = end.saturating_duration_since(tokio::time::Instant::now());
    let wait = (*delay).min(left.saturating_sub(policy.min_read_window));
    if wait.is_zero() {
        return false;
    }
    tokio::time::sleep(wait).await;
    *delay = (*delay * 2).min(policy.max_delay);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    fn params<'a>(format: &'a str, destination: ExportDestination<'a>) -> StartExportParams<'a> {
        StartExportParams {
            workspace_id: "1000000000000000001",
            node_id: "nabcd-efghi-jklmn-opqrs-tuvwx-yz12",
            format,
            version_id: None,
            file_name: None,
            page_size: DEFAULT_PAGE_SIZE,
            margins: DEFAULT_MARGINS,
            comments: DEFAULT_COMMENT_MODE,
            include_resolved: false,
            tracked_changes: false,
            destination,
            idempotency_key: "00000000-0000-4000-8000-000000000001",
        }
    }

    #[test]
    fn start_body_sends_every_option_and_the_key() {
        let mut p = params(
            "docx",
            ExportDestination::Workspace {
                parent_id: Some("f1"),
            },
        );
        p.version_id = Some("v1");
        p.file_name = Some("Report");
        p.comments = "native";
        p.include_resolved = true;
        p.tracked_changes = true;
        assert_eq!(
            start_body(&p),
            serde_json::json!({
                "format": "docx",
                "page_size": "letter",
                "margins": "normal",
                "comments": "native",
                "include_resolved": true,
                "suggestions": "native",
                "idempotency_key": "00000000-0000-4000-8000-000000000001",
                "version_id": "v1",
                "file_name": "Report",
                "destination": "workspace",
                "parent_id": "f1",
            })
        );
        let d = start_body(&params("pdf", ExportDestination::Download));
        assert_eq!(d["destination"], "download");
        assert_eq!(d["suggestions"], "none");
        assert!(d.get("parent_id").is_none());
        assert!(d.get("version_id").is_none());
    }

    #[test]
    fn tracked_changes_need_docx_or_odt() {
        for format in ["pdf", "html", "txt", "md"] {
            let mut p = params(format, ExportDestination::Download);
            p.tracked_changes = true;
            assert!(validate_start(&p).is_err(), "{format}");
        }
        for format in TRACKED_CHANGES_FORMATS {
            let mut p = params(format, ExportDestination::Download);
            p.tracked_changes = true;
            assert!(validate_start(&p).is_ok(), "{format}");
        }
    }

    #[test]
    fn validate_rejects_unknown_values_and_a_blank_key() {
        let mut p = params("rtf", ExportDestination::Download);
        assert!(validate_start(&p).is_err());
        p.format = "pdf";
        p.comments = "inline";
        assert!(validate_start(&p).is_err());
        p.comments = "margin";
        p.idempotency_key = " ";
        assert!(validate_start(&p).is_err());
    }

    #[test]
    fn idempotency_key_is_a_v4_uuid_and_unique() {
        let a = new_idempotency_key().expect("key");
        let b = new_idempotency_key().expect("key");
        assert_ne!(a, b);
        let groups: Vec<&str> = a.split('-').collect();
        assert_eq!(
            groups.iter().map(|g| g.len()).collect::<Vec<_>>(),
            [8, 4, 4, 4, 12]
        );
        assert!(groups[2].starts_with('4'));
        assert!(matches!(
            groups[3].chars().next(),
            Some('8' | '9' | 'a' | 'b')
        ));
    }

    #[test]
    fn export_paths_keep_the_id_opaque() {
        assert_eq!(export_path("ex-1", ""), "/exports/ex-1/");
        assert_eq!(
            export_path(" a/b ", "download/"),
            "/exports/a%2Fb/download/"
        );
    }

    #[test]
    fn record_is_read_from_the_export_wrapper() {
        let payload =
            serde_json::json!({"result": true, "export": {"id": "e1", "status": "queued"}});
        let record = export_record(&payload).expect("record");
        assert_eq!(export_state(record), ExportState::Queued);
        assert_eq!(export_id(record).as_deref(), Some("e1"));
        let nested = serde_json::json!({"response": {"export": {"id": 7, "status": "running"}}});
        let record = export_record(&nested).expect("nested record");
        assert_eq!(export_id(record).as_deref(), Some("7"));
        assert!(export_record(&serde_json::json!({"result": true})).is_none());
    }

    #[test]
    fn failure_error_carries_reason_and_droppable_flag() {
        let record = serde_json::json!({
            "id": "e1", "status": "failed",
            "error": {"reason": "export_too_large", "retryable": false,
                      "params": {"comments_droppable": true}}
        });
        let err = failure_error(&record).expect("failed");
        assert_eq!(err.field_reason(), Some("export_too_large"));
        assert_eq!(err.param_bool("comments_droppable"), Some(true));
        assert!(failure_error(&serde_json::json!({"status": "succeeded"})).is_none());
    }

    /// A fetch closure that serves `statuses` in order (the last repeats) and
    /// counts its calls.
    fn sequence(
        statuses: &[&str],
    ) -> (
        impl FnMut() -> std::future::Ready<Result<Value, CliError>>,
        Arc<Mutex<usize>>,
    ) {
        let queue: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(
            statuses.iter().map(|s| (*s).to_owned()).collect(),
        ));
        let calls = Arc::new(Mutex::new(0usize));
        let counter = Arc::clone(&calls);
        let fetch = move || {
            *counter.lock().expect("lock") += 1;
            let mut q = queue.lock().expect("lock");
            let status = if q.len() > 1 {
                q.pop_front().unwrap_or_default()
            } else {
                q.front().cloned().unwrap_or_default()
            };
            std::future::ready(Ok(serde_json::json!({
                "result": true,
                "export": {"id": "e1", "status": status}
            })))
        };
        (fetch, calls)
    }

    /// Millisecond-scale pacing so the poll tests run in real time quickly.
    /// Sleeps never return early, so lower bounds on time are reliable.
    const FAST: PollPolicy = PollPolicy {
        initial_delay: Duration::from_millis(5),
        max_delay: Duration::from_millis(20),
        request_timeout: Duration::from_millis(100),
        min_read_window: Duration::from_millis(2),
    };

    const LONG: Duration = Duration::from_secs(30);

    #[tokio::test]
    async fn poll_follows_queued_running_succeeded() {
        let (fetch, calls) = sequence(&["running", "running", "succeeded"]);
        let initial = serde_json::json!({"id": "e1", "status": "queued"});
        let out = poll_export(Some(initial), fetch, LONG, FAST)
            .await
            .expect("poll");
        match out {
            PollOutcome::Finished(r) => assert_eq!(export_state(&r), ExportState::Succeeded),
            other @ PollOutcome::DeadlineReached(_) => panic!("{other:?}"),
        }
        assert_eq!(*calls.lock().expect("lock"), 3);
    }

    #[tokio::test]
    async fn poll_makes_no_request_for_a_finished_start() {
        let (fetch, calls) = sequence(&["running"]);
        let initial = serde_json::json!({"id": "e1", "status": "succeeded",
            "output": {"node_id": "n2"}});
        let out = poll_export(Some(initial), fetch, LONG, FAST)
            .await
            .expect("poll");
        assert!(matches!(out, PollOutcome::Finished(_)));
        assert_eq!(*calls.lock().expect("lock"), 0);
    }

    #[tokio::test]
    async fn poll_stops_on_failed_without_resubmitting() {
        let (fetch, calls) = sequence(&["running", "failed"]);
        let out = poll_export(None, fetch, LONG, FAST).await.expect("poll");
        match out {
            PollOutcome::Finished(r) => assert_eq!(export_state(&r), ExportState::Failed),
            other @ PollOutcome::DeadlineReached(_) => panic!("{other:?}"),
        }
        assert_eq!(*calls.lock().expect("lock"), 2);
    }

    #[tokio::test]
    async fn poll_backs_off_to_the_cap_and_stops_at_the_deadline() {
        let stamps: Arc<Mutex<Vec<tokio::time::Instant>>> = Arc::default();
        let sink = Arc::clone(&stamps);
        let fetch = move || {
            sink.lock().expect("lock").push(tokio::time::Instant::now());
            std::future::ready(Ok(
                serde_json::json!({"export": {"id": "e1", "status": "running"}}),
            ))
        };
        let deadline = Duration::from_millis(400);
        let started = tokio::time::Instant::now();
        let out = poll_export(None, fetch, deadline, FAST)
            .await
            .expect("poll");
        let elapsed = started.elapsed();
        assert!(matches!(out, PollOutcome::DeadlineReached(Some(_))));
        assert!(elapsed >= deadline.saturating_sub(FAST.min_read_window + FAST.max_delay));
        assert!(
            elapsed < deadline + Duration::from_millis(200),
            "{elapsed:?}"
        );
        let stamps = stamps.lock().expect("lock").clone();
        let gaps: Vec<Duration> = stamps.windows(2).map(|w| w[1] - w[0]).collect();
        // Doubling from 5 ms: each of the first gaps is at least its delay.
        for (gap, at_least) in gaps.iter().zip([5u64, 10, 20, 20]) {
            assert!(*gap >= Duration::from_millis(at_least), "{gaps:?}");
        }
        // Capped at 20 ms: about 20 reads fit in 400 ms. An uncapped doubling
        // would fit 7, no backoff about 80.
        assert!((12..=40).contains(&stamps.len()), "{}", stamps.len());
    }

    #[tokio::test]
    async fn zero_deadline_is_a_single_check() {
        let (fetch, calls) = sequence(&["running"]);
        let out = poll_export(None, fetch, Duration::ZERO, FAST)
            .await
            .expect("poll");
        assert!(matches!(out, PollOutcome::DeadlineReached(Some(_))));
        assert_eq!(*calls.lock().expect("lock"), 1);
    }

    #[tokio::test]
    async fn a_queued_start_with_no_time_left_reads_nothing() {
        let (fetch, calls) = sequence(&["running"]);
        let initial = serde_json::json!({"id": "e1", "status": "queued"});
        let out = poll_export(Some(initial.clone()), fetch, Duration::ZERO, FAST)
            .await
            .expect("poll");
        assert_eq!(out, PollOutcome::DeadlineReached(Some(initial)));
        assert_eq!(*calls.lock().expect("lock"), 0);
    }

    #[tokio::test]
    async fn a_hung_read_is_bounded() {
        let calls = Arc::new(Mutex::new(0usize));
        let counter = Arc::clone(&calls);
        let fetch = move || {
            *counter.lock().expect("lock") += 1;
            std::future::pending::<Result<Value, CliError>>()
        };
        let started = tokio::time::Instant::now();
        let out = poll_export(None, fetch, Duration::from_millis(250), FAST)
            .await
            .expect("poll");
        assert_eq!(out, PollOutcome::DeadlineReached(None));
        assert!(
            started.elapsed() < Duration::from_millis(250) + FAST.request_timeout,
            "{:?}",
            started.elapsed()
        );
        assert!(*calls.lock().expect("lock") >= 2);
    }

    /// With a positive deadline, even the first read (no record in hand) is
    /// bounded by the time left, not by the full request timeout.
    #[tokio::test]
    async fn the_first_read_is_bounded_by_a_positive_deadline() {
        let calls = Arc::new(Mutex::new(0usize));
        let counter = Arc::clone(&calls);
        let fetch = move || {
            *counter.lock().expect("lock") += 1;
            std::future::pending::<Result<Value, CliError>>()
        };
        let policy = PollPolicy {
            request_timeout: Duration::from_secs(5),
            ..FAST
        };
        let started = tokio::time::Instant::now();
        let out = poll_export(None, fetch, Duration::from_millis(60), policy)
            .await
            .expect("poll");
        assert_eq!(out, PollOutcome::DeadlineReached(None));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(*calls.lock().expect("lock"), 1);
    }

    #[tokio::test]
    async fn an_unrepresentable_deadline_is_an_error_not_a_panic() {
        let (fetch, calls) = sequence(&["running"]);
        let out = poll_export(None, fetch, Duration::MAX, FAST).await;
        assert!(out.is_err());
        assert_eq!(*calls.lock().expect("lock"), 0);
    }

    #[tokio::test]
    async fn a_failed_read_is_returned() {
        let fetch =
            || std::future::ready(Err::<Value, CliError>(CliError::Parse("boom".to_owned())));
        let out = poll_export(None, fetch, LONG, FAST).await;
        assert!(out.is_err());
    }
}
