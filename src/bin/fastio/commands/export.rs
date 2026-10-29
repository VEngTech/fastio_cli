//! `fastio export` — server-side note export.
//!
//! Starting an export enqueues a job; the command then polls it with capped
//! backoff until it finishes or `--timeout` passes, and downloads the file or
//! reports the saved copy. A failed job is reported with its reason and never
//! started again. `fastio export status <id>` picks up a job started with
//! `--no-wait` (or one that outlived its timeout).

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use colored::Colorize;
use serde_json::Value;

use super::CommandContext;
use crate::cli::{ExportArgs, ExportCommands};
use fastio_cli::api::export::{self as api_export, ExportDestination, ExportState, PollOutcome};
use fastio_cli::client::ApiClient;
use fastio_cli::output::OutputFormat;

/// Most report entries named on the one-line stderr summary, per list.
const REPORT_IDS_SHOWN: usize = 5;

/// Execute `fastio export …` or `fastio export status …`.
pub async fn execute(args: ExportArgs, ctx: &CommandContext<'_>) -> Result<()> {
    match args.command {
        Some(ExportCommands::Status {
            export_id,
            output,
            timeout,
        }) => {
            if let Some(path) = output.as_deref() {
                check_output_path(path)?;
            }
            let client = ctx.build_client()?;
            wait_and_finish(
                ctx,
                &client,
                None,
                export_id.trim(),
                output.as_deref(),
                Duration::from_secs(timeout),
            )
            .await
        }
        None => start(args, ctx).await,
    }
}

/// Start an export, then wait for it unless `--no-wait`.
async fn start(args: ExportArgs, ctx: &CommandContext<'_>) -> Result<()> {
    let (Some(node_id), Some(workspace_id), Some(format)) = (
        args.node_id.as_deref(),
        args.workspace.as_deref(),
        args.to.as_deref(),
    ) else {
        anyhow::bail!("give the note's node ID, --workspace and --to");
    };
    let idempotency_key = api_export::new_idempotency_key()?;
    let destination = if args.save_copy {
        ExportDestination::Workspace {
            parent_id: args.folder.as_deref(),
        }
    } else {
        ExportDestination::Download
    };
    let params = api_export::StartExportParams {
        workspace_id,
        node_id,
        format,
        version_id: args.version.as_deref(),
        file_name: args.name.as_deref(),
        page_size: &args.page_size,
        margins: &args.margins,
        comments: &args.comments,
        include_resolved: args.include_resolved,
        tracked_changes: args.tracked_changes,
        destination,
        idempotency_key: &idempotency_key,
    };
    // Judge everything the client can before any request: a bad option or an
    // unusable --output must not cost a server-side render.
    api_export::validate_start(&params)?;
    if let Some(path) = args.output.as_deref() {
        check_output_path(path)?;
    }

    let client = ctx.build_client()?;
    let payload = api_export::start_export(&client, &params)
        .await
        .context("failed to start the export")?;
    let record = api_export::export_record(&payload)
        .cloned()
        .context("the server's reply carries no export record")?;
    let export_id =
        api_export::export_id(&record).context("the server's reply carries no export ID")?;

    // A replayed start can return a job that has already failed; that is a
    // failure even under --no-wait.
    if api_export::export_state(&record) == ExportState::Failed {
        return fail(ctx, &record, &export_id);
    }
    if args.no_wait {
        if !ctx.output.quiet {
            eprintln!(
                "export {export_id} started; check it with `fastio export status {export_id}`"
            );
        }
        return render_record(ctx, record);
    }
    wait_and_finish(
        ctx,
        &client,
        Some(record),
        &export_id,
        args.output.as_deref(),
        Duration::from_secs(args.timeout),
    )
    .await
}

/// Poll until the export finishes (or the deadline passes), then download the
/// file, report the saved copy, or surface the failure.
async fn wait_and_finish(
    ctx: &CommandContext<'_>,
    client: &ApiClient,
    initial: Option<Value>,
    export_id: &str,
    output: Option<&Path>,
    deadline: Duration,
) -> Result<()> {
    let outcome = api_export::poll_export(
        initial,
        || api_export::export_status(client, export_id),
        deadline,
        api_export::PollPolicy::DEFAULT,
    )
    .await
    .with_context(|| format!("failed to check export {export_id}"))?;

    let record = match outcome {
        PollOutcome::Finished(record) => record,
        PollOutcome::DeadlineReached(last) => {
            let state = last
                .as_ref()
                .and_then(|r| r.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            anyhow::bail!(
                "export {export_id} is still {state} after {}s; check it later with \
                 `fastio export status {export_id}`",
                deadline.as_secs()
            );
        }
        // `PollOutcome` is non-exhaustive; nothing else is produced today.
        _ => anyhow::bail!("export {export_id} ended in an unknown state"),
    };

    match api_export::export_state(&record) {
        ExportState::Failed => fail(ctx, &record, export_id),
        _ => finish_succeeded(ctx, client, record, export_id, output).await,
    }
}

/// A failed export: the report on stderr, then the reason (with its hint) as
/// the error, so the command exits non-zero.
fn fail(ctx: &CommandContext<'_>, record: &Value, export_id: &str) -> Result<()> {
    print_report(ctx, record);
    let err =
        api_export::failure_error(record).context("the export failed without an error record")?;
    Err(anyhow::Error::from(fastio_cli::error::CliError::Api(err)))
        .with_context(|| format!("export {export_id} failed"))
}

/// Print a record's report summary on stderr (unless `--quiet`). For the paths
/// that render no record, so the report is not lost.
fn print_report(ctx: &CommandContext<'_>, record: &Value) {
    if !ctx.output.quiet
        && let Some(line) = record.get("report").and_then(report_summary)
    {
        eprintln!("{line}");
    }
}

/// A finished export: download it, or report where it was saved.
async fn finish_succeeded(
    ctx: &CommandContext<'_>,
    client: &ApiClient,
    mut record: Value,
    export_id: &str,
    output: Option<&Path>,
) -> Result<()> {
    if api_export::saved_node_id(&record).is_some() {
        if output.is_some() && !ctx.output.quiet {
            eprintln!(
                "{} this export was saved in the workspace, so nothing was downloaded",
                "warning:".yellow().bold()
            );
        }
        return render_record(ctx, record);
    }
    let Some(path) = output else {
        if !ctx.output.quiet {
            eprintln!(
                "export {export_id} is ready; download it with \
                 `fastio export status {export_id} --output <path>`"
            );
        }
        return render_record(ctx, record);
    };
    let bytes = match api_export::download_export(client, export_id, path).await {
        Ok(bytes) => bytes,
        Err(e) => {
            // Nothing is rendered on this path, so the report would be lost.
            print_report(ctx, &record);
            return Err(anyhow::Error::from(e))
                .with_context(|| format!("failed to download export {export_id}"));
        }
    };
    if let Some(obj) = record.as_object_mut() {
        obj.insert(
            "saved_to".to_owned(),
            Value::from(path.display().to_string()),
        );
        obj.insert("bytes".to_owned(), Value::from(bytes));
    }
    render_record(ctx, record)
}

/// Refuse an `--output` that cannot be written before any request is made:
/// a directory, or a path whose parent directory does not exist.
fn check_output_path(path: &Path) -> Result<()> {
    anyhow::ensure!(
        !path.is_dir(),
        "--output must be a file path, not a directory: {}",
        path.display()
    );
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        anyhow::ensure!(
            parent.is_dir(),
            "--output's directory does not exist: {}",
            parent.display()
        );
    }
    Ok(())
}

/// Render an export record. For table and csv, the `report` moves to one
/// stderr line so the tabular output stays flat; json and markdown keep it
/// inline.
fn render_record(ctx: &CommandContext<'_>, mut record: Value) -> Result<()> {
    if matches!(ctx.output.format, OutputFormat::Table | OutputFormat::Csv)
        && let Some(report) = record.as_object_mut().and_then(|o| o.remove("report"))
        && !ctx.output.quiet
        && let Some(line) = report_summary(&report)
    {
        eprintln!("{line}");
    }
    ctx.output.render(&record)?;
    Ok(())
}

/// One line describing an export report, or `None` when there is none.
fn report_summary(report: &Value) -> Option<String> {
    let obj = report.as_object()?;
    let mut parts = Vec::new();
    if let Some(pages) = obj.get("pages").and_then(Value::as_u64) {
        parts.push(format!("{pages} pages"));
    }
    for (key, label) in [
        ("orphaned_threads", "comment threads not placed"),
        ("unplaced_suggestions", "suggestions not placed"),
        ("missing_assets", "images missing"),
        ("dropped", "items dropped"),
    ] {
        let Some(list) = obj.get(key).and_then(Value::as_array) else {
            continue;
        };
        if list.is_empty() {
            continue;
        }
        let mut shown: Vec<String> = list
            .iter()
            .take(REPORT_IDS_SHOWN)
            .map(report_entry)
            .collect();
        if list.len() > REPORT_IDS_SHOWN {
            shown.push("…".to_owned());
        }
        parts.push(format!("{} {label} ({})", list.len(), shown.join(", ")));
    }
    if obj.get("truncated").and_then(Value::as_bool) == Some(true) {
        parts.push("report truncated".to_owned());
    }
    if parts.is_empty() {
        parts.push("nothing dropped".to_owned());
    }
    Some(format!("export report: {}", parts.join("; ")))
}

/// One report entry: a string as-is, or `{id, reason}` as `id: reason`.
/// Control characters are dropped, since the text comes from the server.
fn report_entry(entry: &Value) -> String {
    let text = match entry {
        Value::String(s) => s.clone(),
        Value::Object(o) => {
            let id = o.get("id").map_or_else(String::new, scalar_text);
            match o.get("reason").and_then(Value::as_str) {
                Some(reason) => format!("{id}: {reason}"),
                None => id,
            }
        }
        other => scalar_text(other),
    };
    text.chars().filter(|c| !c.is_control()).collect()
}

/// A JSON scalar as plain text (strings without quotes).
fn scalar_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_summary_names_what_was_not_placed() {
        let report = serde_json::json!({
            "pages": 3,
            "orphaned_threads": [{"id": "c1", "reason": "stale"}, {"id": "c2", "reason": "lost"}],
            "unplaced_suggestions": [],
            "missing_assets": ["a1"],
            "dropped": [],
            "truncated": true
        });
        assert_eq!(
            report_summary(&report).as_deref(),
            Some(
                "export report: 3 pages; 2 comment threads not placed (c1: stale, c2: lost); \
                 1 images missing (a1); report truncated"
            )
        );
    }

    #[test]
    fn report_summary_of_a_clean_export_and_of_none() {
        let clean = serde_json::json!({"pages": null, "orphaned_threads": [], "truncated": false});
        assert_eq!(
            report_summary(&clean).as_deref(),
            Some("export report: nothing dropped")
        );
        assert_eq!(report_summary(&Value::Null), None);
    }

    #[test]
    fn report_summary_caps_long_lists_and_strips_control_characters() {
        let many: Vec<Value> = (0..8).map(|i| Value::from(format!("x{i}"))).collect();
        let report = serde_json::json!({"dropped": many, "missing_assets": ["a\u{1b}[31mb"]});
        let line = report_summary(&report).expect("line");
        assert!(
            line.contains("8 items dropped (x0, x1, x2, x3, x4, …)"),
            "{line}"
        );
        assert!(line.contains("(a[31mb)"), "{line}");
    }

    #[test]
    fn output_path_must_be_a_file_in_an_existing_directory() {
        let dir = std::env::temp_dir();
        assert!(check_output_path(&dir).is_err());
        assert!(check_output_path(&dir.join("missing-dir-for-export").join("out.pdf")).is_err());
        assert!(check_output_path(&dir.join("out.pdf")).is_ok());
        assert!(check_output_path(Path::new("out.pdf")).is_ok());
    }
}
