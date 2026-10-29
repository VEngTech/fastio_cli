//! Wire-level pins for note export: the start body and its idempotency key,
//! the status poll, and the download.
//!
//! The download carries the bearer token, so its sharpest guarantees are
//! negative: a redirect is never followed, a JSON body is never written as the
//! file, and a failed or interrupted download never leaves a partial file or
//! clobbers one that was already there. The start is replay-safe only because
//! every replay carries the same idempotency key, so the tests count requests
//! and compare keys.
//!
//! The isolation measures for the binary runs follow
//! `tests/service_account_wire.rs`.

use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::{Arc, Mutex};

use fastio_cli::api::export::{self, ExportDestination, StartExportParams};
use fastio_cli::client::ApiClient;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const WORKSPACE: &str = "1000000000000000001";
const NODE: &str = "nabcd-efghi-jklmn-opqrs-tuvwx-yz12";
const NODE_RAW: &str = "nabcdefghijklmnopqrstuvwxyz12";
const START_PATH: &str = "/workspace/";
const STATUS_PATH: &str = "/exports/e1/";
const DOWNLOAD_PATH: &str = "/exports/e1/download/";
const PDF_BYTES: &[u8] = b"%PDF-1.7 synthetic export bytes";

/// One canned reply. `status == 0` drops the connection after reading the
/// request. `declared_len` overrides `Content-Length`, so a body shorter than
/// declared is an interrupted download. `location` is sent as `Location` on a
/// 3xx (default: the request's own path).
#[derive(Clone)]
struct Reply {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
    declared_len: Option<usize>,
    location: Option<String>,
}

fn json_reply(status: u16, body: &Value) -> Reply {
    Reply {
        status,
        content_type: "application/json",
        body: body.to_string().into_bytes(),
        declared_len: None,
        location: None,
    }
}

fn file_reply(body: &[u8]) -> Reply {
    Reply {
        status: 200,
        content_type: "application/pdf",
        body: body.to_vec(),
        declared_len: None,
        location: None,
    }
}

fn record(status: &str) -> Value {
    json!({"result": true, "export": {"id": "e1", "status": status}})
}

fn with_record(fields: &Value) -> Value {
    let mut export = json!({"id": "e1"});
    if let (Some(base), Some(extra)) = (export.as_object_mut(), fields.as_object()) {
        for (k, v) in extra {
            base.insert(k.clone(), v.clone());
        }
    }
    json!({"result": true, "export": export})
}

/// What the stub saw: `(request line, lower-cased head, body)`.
#[derive(Clone, Default)]
struct Seen {
    requests: Arc<Mutex<Vec<(String, String, String)>>>,
}

impl Seen {
    fn all(&self) -> Vec<(String, String, String)> {
        self.requests.lock().map(|r| r.clone()).unwrap_or_default()
    }

    fn lines(&self) -> Vec<String> {
        self.all().into_iter().map(|(line, _, _)| line).collect()
    }
}

/// Serve every connection: a request goes to the FIRST route whose prefix its
/// path starts with, and gets that route's replies in order (the last
/// repeats). List longer prefixes first.
async fn spawn_stub(routes: Vec<(&'static str, Vec<Reply>)>) -> (String, Seen) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr").to_string();
    let seen = Seen::default();
    let task_seen = seen.clone();
    let cursors: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(vec![0; routes.len()]));
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut raw = Vec::new();
            let mut buf = [0u8; 8192];
            let header_end = loop {
                let Ok(n) = sock.read(&mut buf).await else {
                    break None;
                };
                if n == 0 {
                    break None;
                }
                raw.extend_from_slice(&buf[..n]);
                if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(pos + 4);
                }
            };
            let Some(header_end) = header_end else {
                continue;
            };
            let head = String::from_utf8_lossy(&raw[..header_end]).to_string();
            let content_length = head
                .lines()
                .find_map(|l| {
                    let (name, value) = l.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            while raw.len() < header_end + content_length {
                match sock.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => raw.extend_from_slice(&buf[..n]),
                }
            }
            let mut parts = head.lines().next().unwrap_or_default().split_whitespace();
            let method = parts.next().unwrap_or_default().to_owned();
            let path = parts.next().unwrap_or_default().to_owned();
            let body = String::from_utf8_lossy(&raw[header_end..]).to_string();
            if let Ok(mut r) = task_seen.requests.lock() {
                r.push((format!("{method} {path}"), head.to_ascii_lowercase(), body));
            }
            let chosen = routes
                .iter()
                .position(|(prefix, _)| path.starts_with(prefix))
                .and_then(|i| {
                    let mut cursors = cursors.lock().ok()?;
                    let replies = &routes[i].1;
                    let at = cursors[i].min(replies.len().saturating_sub(1));
                    cursors[i] += 1;
                    replies.get(at).cloned()
                });
            let Some(chosen) = chosen else {
                continue;
            };
            if chosen.status == 0 {
                drop(sock);
                continue;
            }
            // A redirect points back at the same path unless told otherwise, so
            // following it would show up as a second request.
            let location = if (300..400).contains(&chosen.status) {
                format!(
                    "Location: {}\r\n",
                    chosen.location.as_deref().unwrap_or(&path)
                )
            } else {
                String::new()
            };
            let header = format!(
                "HTTP/1.1 {} X\r\nContent-Type: {}\r\n{location}Content-Length: {}\r\n\
                 Connection: close\r\n\r\n",
                chosen.status,
                chosen.content_type,
                chosen.declared_len.unwrap_or(chosen.body.len()),
            );
            let _ = sock.write_all(header.as_bytes()).await;
            let _ = sock.write_all(&chosen.body).await;
            let _ = sock.flush().await;
        }
    });
    (addr, seen)
}

fn client(addr: &str) -> ApiClient {
    ApiClient::new_without_proxy(&format!("http://{addr}"), Some("tok".to_owned()))
        .expect("client builds")
}

fn start_params<'a>(key: &'a str, destination: ExportDestination<'a>) -> StartExportParams<'a> {
    StartExportParams {
        workspace_id: WORKSPACE,
        node_id: NODE,
        format: "docx",
        version_id: Some("v3"),
        file_name: Some("Plan"),
        page_size: "a4",
        margins: "wide",
        comments: "native",
        include_resolved: true,
        tracked_changes: true,
        destination,
        idempotency_key: key,
    }
}

fn body_json(body: &str) -> Value {
    serde_json::from_str(body).expect("request body is JSON")
}

/// A fresh, empty scratch directory for one test.
fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("fastio-export-wire-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// Every file name in `dir`.
fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read scratch dir")
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
        .collect();
    names.sort();
    names
}

// ─── Start: body and idempotency key ──────────────────────────────────────────

#[tokio::test]
async fn start_sends_every_option_and_the_idempotency_key() {
    let (addr, seen) =
        spawn_stub(vec![(START_PATH, vec![json_reply(202, &record("queued"))])]).await;
    let key = export::new_idempotency_key().expect("key");
    let payload = export::start_export(
        &client(&addr),
        &start_params(
            &key,
            ExportDestination::Workspace {
                parent_id: Some("f1"),
            },
        ),
    )
    .await
    .expect("start");
    assert_eq!(
        export::export_record(&payload).and_then(export::export_id),
        Some("e1".to_owned())
    );
    let requests = seen.all();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].0,
        format!("POST /workspace/{WORKSPACE}/storage/{NODE_RAW}/export/")
    );
    assert!(requests[0].1.contains("content-type: application/json"));
    assert_eq!(
        body_json(&requests[0].2),
        json!({
            "format": "docx",
            "page_size": "a4",
            "margins": "wide",
            "comments": "native",
            "include_resolved": true,
            "suggestions": "native",
            "idempotency_key": key,
            "version_id": "v3",
            "file_name": "Plan",
            "destination": "workspace",
            "parent_id": "f1",
        })
    );
}

/// A gateway error is replayed with the SAME key, so the server returns the
/// same export instead of starting a second one.
#[tokio::test]
async fn a_replayed_start_reuses_the_key() {
    let (addr, seen) = spawn_stub(vec![(
        START_PATH,
        vec![
            json_reply(503, &json!({"result": false, "error": {"text": "busy"}})),
            json_reply(202, &record("queued")),
        ],
    )])
    .await;
    let key = export::new_idempotency_key().expect("key");
    export::start_export(
        &client(&addr),
        &start_params(&key, ExportDestination::Download),
    )
    .await
    .expect("start after one replay");
    let requests = seen.all();
    assert_eq!(requests.len(), 2, "{:?}", seen.lines());
    let keys: Vec<Value> = requests
        .iter()
        .map(|(_, _, body)| body_json(body)["idempotency_key"].clone())
        .collect();
    assert_eq!(keys, [json!(key), json!(key)]);
}

// ─── Download: what is never written ──────────────────────────────────────────

#[tokio::test]
async fn download_writes_the_file_with_the_bearer_and_no_query() {
    let (addr, seen) = spawn_stub(vec![(DOWNLOAD_PATH, vec![file_reply(PDF_BYTES)])]).await;
    let dir = scratch("ok");
    let out = dir.join("plan.pdf");
    let bytes = export::download_export(&client(&addr), "e1", &out)
        .await
        .expect("download");
    assert_eq!(bytes, PDF_BYTES.len() as u64);
    assert_eq!(std::fs::read(&out).expect("read"), PDF_BYTES);
    assert_eq!(listing(&dir), ["plan.pdf"]);
    let requests = seen.all();
    assert_eq!(requests[0].0, format!("GET {DOWNLOAD_PATH}"));
    assert!(requests[0].1.contains("authorization: bearer tok"));
}

#[tokio::test]
async fn download_never_follows_a_redirect() {
    for status in [302, 307] {
        // The redirect target is a second origin (another port) that would
        // happily serve the file; following the redirect would reach it.
        let (elsewhere, elsewhere_seen) =
            spawn_stub(vec![("/", vec![file_reply(PDF_BYTES)])]).await;
        let redirect = Reply {
            status,
            content_type: "text/plain",
            body: Vec::new(),
            declared_len: None,
            location: Some(format!("http://{elsewhere}/files/plan.pdf")),
        };
        let (addr, seen) = spawn_stub(vec![(DOWNLOAD_PATH, vec![redirect])]).await;
        let dir = scratch(&format!("redirect-{status}"));
        let out = dir.join("plan.pdf");
        let err = export::download_export(&client(&addr), "e1", &out)
            .await
            .expect_err("a redirect is refused");
        assert!(err.to_string().contains("redirect"), "{err}");
        assert_eq!(seen.all().len(), 1, "{status}");
        let reached = elsewhere_seen.all();
        assert!(reached.is_empty(), "{status}: the redirect was followed");
        assert!(
            !reached
                .iter()
                .any(|(_, head, _)| head.contains("authorization")),
            "{status}: the bearer reached the redirect target"
        );
        assert!(listing(&dir).is_empty(), "{status}: {:?}", listing(&dir));
    }

    // Positive control: a redirect-following client given the same reply DOES
    // reach the second origin, so the zero above is a real measurement.
    let (elsewhere, elsewhere_seen) = spawn_stub(vec![("/", vec![file_reply(PDF_BYTES)])]).await;
    let redirect = Reply {
        status: 302,
        content_type: "text/plain",
        body: Vec::new(),
        declared_len: None,
        location: Some(format!("http://{elsewhere}/files/plan.pdf")),
    };
    let (addr, _seen) = spawn_stub(vec![(DOWNLOAD_PATH, vec![redirect])]).await;
    let following = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("client builds");
    let resp = following
        .get(format!("http://{addr}{DOWNLOAD_PATH}"))
        .send()
        .await
        .expect("followed");
    assert!(resp.status().is_success());
    assert_eq!(elsewhere_seen.all().len(), 1);
}

#[tokio::test]
async fn download_never_writes_a_json_body_as_the_file() {
    let envelope = json_reply(200, &json!({"result": true, "export": {"id": "e1"}}));
    let refusal = json_reply(
        200,
        &json!({"result": false, "error": {"code": 1, "text": "gone",
            "params": {"reason": "export_download_expired"}}}),
    );
    for (label, reply, reason) in [
        ("success envelope", envelope, None),
        ("error envelope", refusal, Some("export_download_expired")),
    ] {
        let (addr, _seen) = spawn_stub(vec![(DOWNLOAD_PATH, vec![reply])]).await;
        let dir = scratch(&format!("json-{}", label.replace(' ', "-")));
        let out = dir.join("plan.pdf");
        let err = export::download_export(&client(&addr), "e1", &out)
            .await
            .expect_err("a JSON body is refused");
        if let Some(reason) = reason {
            assert!(err.to_string().contains(reason), "{label}: {err}");
        } else {
            assert!(
                err.to_string().contains("nothing was written"),
                "{label}: {err}"
            );
        }
        assert!(listing(&dir).is_empty(), "{label}: {:?}", listing(&dir));
    }
}

/// A body cut short, or an error status, leaves no partial file and keeps the
/// file already at `--output`.
#[tokio::test]
async fn a_failed_download_keeps_the_existing_file_and_leaves_no_partial() {
    let interrupted = Reply {
        status: 200,
        content_type: "application/pdf",
        body: b"%PDF-1.7 trunc".to_vec(),
        declared_len: Some(100_000),
        location: None,
    };
    let not_found = json_reply(
        404,
        &json!({"result": false, "error": {"code": 1, "text": "not found"}}),
    );
    for (label, reply) in [("interrupted", interrupted), ("not-found", not_found)] {
        let (addr, _seen) = spawn_stub(vec![(DOWNLOAD_PATH, vec![reply])]).await;
        let dir = scratch(label);
        let out = dir.join("plan.pdf");
        std::fs::write(&out, b"previous contents").expect("seed existing file");
        export::download_export(&client(&addr), "e1", &out)
            .await
            .expect_err("the download fails");
        assert_eq!(
            std::fs::read(&out).expect("read"),
            b"previous contents",
            "{label}"
        );
        assert_eq!(listing(&dir), ["plan.pdf"], "{label}");
    }
}

// ─── The command: poll, finish, fail ──────────────────────────────────────────

fn start_args<'a>(out: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        "export",
        NODE,
        "--workspace",
        WORKSPACE,
        "--to",
        "pdf",
        "--output",
        out,
    ];
    args.extend_from_slice(extra);
    args
}

#[tokio::test]
async fn queued_running_succeeded_then_downloads() {
    let (addr, seen) = spawn_stub(vec![
        (DOWNLOAD_PATH, vec![file_reply(PDF_BYTES)]),
        (
            STATUS_PATH,
            vec![
                json_reply(200, &record("running")),
                json_reply(
                    200,
                    &with_record(&json!({"status": "succeeded",
                        "output": {"download_url": "https://files.example/x", "expires": "2030-01-01"},
                        "report": {"pages": 2, "orphaned_threads": []}})),
                ),
            ],
        ),
        (START_PATH, vec![json_reply(202, &record("queued"))]),
    ])
    .await;
    let dir = scratch("flow");
    let out = dir.join("plan.pdf");
    let out_s = out.display().to_string();
    let run = run(&addr, &["--format", "json"], &start_args(&out_s, &[])).await;
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(std::fs::read(&out).expect("read"), PDF_BYTES);
    let lines = seen.lines();
    assert_eq!(
        lines,
        [
            format!("POST /workspace/{WORKSPACE}/storage/{NODE_RAW}/export/"),
            format!("GET {STATUS_PATH}"),
            format!("GET {STATUS_PATH}"),
            format!("GET {DOWNLOAD_PATH}"),
        ]
    );
    let printed: Value = serde_json::from_slice(&run.stdout).expect("json output");
    assert_eq!(printed["status"], "succeeded");
    assert_eq!(printed["saved_to"], out_s);
    assert_eq!(printed["report"]["pages"], 2);
}

#[tokio::test]
async fn md_finishes_in_the_start_response_and_downloads() {
    let (addr, seen) = spawn_stub(vec![
        (DOWNLOAD_PATH, vec![file_reply(b"# Plan\n")]),
        (
            START_PATH,
            vec![json_reply(
                200,
                &with_record(&json!({"status": "succeeded", "output": {"expires": "2030-01-01"}})),
            )],
        ),
    ])
    .await;
    let dir = scratch("md");
    let out = dir.join("plan.md");
    let out_s = out.display().to_string();
    let args = [
        "export",
        NODE,
        "--workspace",
        WORKSPACE,
        "--to",
        "md",
        "--output",
        &out_s,
    ];
    let run = run(&addr, &[], &args).await;
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(std::fs::read(&out).expect("read"), b"# Plan\n");
    assert_eq!(seen.lines().len(), 2, "{:?}", seen.lines());
}

#[tokio::test]
async fn save_copy_prints_the_new_node_and_downloads_nothing() {
    let (addr, seen) = spawn_stub(vec![
        (
            STATUS_PATH,
            vec![json_reply(
                200,
                &with_record(
                    &json!({"status": "succeeded", "output": {"node_id": "nsavedcopy"},
                    "report": {"orphaned_threads": [{"id": "c9", "reason": "lost"}]}}),
                ),
            )],
        ),
        (START_PATH, vec![json_reply(202, &record("queued"))]),
    ])
    .await;
    let args = [
        "export",
        NODE,
        "--workspace",
        WORKSPACE,
        "--to",
        "docx",
        "--save-copy",
        "--folder",
        "f1",
    ];
    let run = run(&addr, &["--format", "table"], &args).await;
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(String::from_utf8_lossy(&run.stdout).contains("nsavedcopy"));
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr.contains("export report:") && stderr.contains("c9: lost"),
        "{stderr}"
    );
    assert_eq!(stderr.matches("export report:").count(), 1, "{stderr}");
    let requests = seen.all();
    assert_eq!(requests.len(), 2, "{:?}", seen.lines());
    let body = body_json(&requests[0].2);
    assert_eq!(body["destination"], "workspace");
    assert_eq!(body["parent_id"], "f1");
}

/// A failed job exits non-zero with its reason's hint, and is never started
/// again — retryable or not.
#[tokio::test]
async fn a_failed_job_is_reported_and_never_restarted() {
    for (error, hint) in [
        (
            json!({"reason": "export_timeout", "retryable": true}),
            "took too long",
        ),
        (
            json!({"reason": "export_too_large", "retryable": false,
                   "params": {"comments_droppable": true}}),
            "--comments none",
        ),
    ] {
        let (addr, seen) = spawn_stub(vec![
            (
                STATUS_PATH,
                vec![json_reply(
                    200,
                    &with_record(&json!({"status": "failed", "error": error})),
                )],
            ),
            (START_PATH, vec![json_reply(202, &record("queued"))]),
        ])
        .await;
        let dir = scratch("failed");
        let out = dir.join("plan.pdf");
        let out_s = out.display().to_string();
        let run = run(&addr, &[], &start_args(&out_s, &[])).await;
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert_eq!(run.status.code(), Some(1), "{stderr}");
        assert!(stderr.contains("export e1 failed"), "{stderr}");
        assert!(stderr.contains(hint), "{hint}: {stderr}");
        let posts = seen
            .lines()
            .iter()
            .filter(|l| l.starts_with("POST"))
            .count();
        assert_eq!(posts, 1, "{:?}", seen.lines());
        assert!(listing(&dir).is_empty());
    }
}

#[tokio::test]
async fn the_deadline_prints_the_export_id() {
    let (addr, seen) =
        spawn_stub(vec![(START_PATH, vec![json_reply(202, &record("queued"))])]).await;
    let dir = scratch("deadline");
    let out = dir.join("plan.pdf");
    let out_s = out.display().to_string();
    let run = run(&addr, &[], &start_args(&out_s, &["--timeout", "0"])).await;
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert_eq!(run.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("fastio export status e1"), "{stderr}");
    assert_eq!(seen.lines().len(), 1);
}

#[tokio::test]
async fn no_wait_then_status_downloads() {
    let (addr, seen) = spawn_stub(vec![
        (DOWNLOAD_PATH, vec![file_reply(PDF_BYTES)]),
        (
            STATUS_PATH,
            vec![json_reply(
                200,
                &with_record(&json!({"status": "succeeded", "output": {"expires": "2030-01-01"}})),
            )],
        ),
        (START_PATH, vec![json_reply(202, &record("queued"))]),
    ])
    .await;
    let args = [
        "export",
        NODE,
        "--workspace",
        WORKSPACE,
        "--to",
        "pdf",
        "--no-wait",
    ];
    let started = run(&addr, &["--format", "json"], &args).await;
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    let printed: Value = serde_json::from_slice(&started.stdout).expect("json output");
    assert_eq!(printed["id"], "e1");
    assert_eq!(seen.lines().len(), 1);
    assert_eq!(body_json(&seen.all()[0].2)["destination"], "download");

    let dir = scratch("resume");
    let out = dir.join("plan.pdf");
    let out_s = out.display().to_string();
    let resumed = run(&addr, &[], &["export", "status", "e1", "--output", &out_s]).await;
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    assert_eq!(std::fs::read(&out).expect("read"), PDF_BYTES);
    assert_eq!(
        seen.lines()[1..],
        [format!("GET {STATUS_PATH}"), format!("GET {DOWNLOAD_PATH}")]
    );
}

/// A replayed start can return a job that already failed: `--no-wait` must
/// still exit non-zero with the report and the reason's hint.
#[tokio::test]
async fn no_wait_on_an_already_failed_job_exits_non_zero() {
    let (addr, seen) = spawn_stub(vec![(
        START_PATH,
        vec![json_reply(
            200,
            &with_record(&json!({"status": "failed",
                "error": {"reason": "export_render_failed", "retryable": false},
                "report": {"missing_assets": ["a1"]}})),
        )],
    )])
    .await;
    let args = [
        "export",
        NODE,
        "--workspace",
        WORKSPACE,
        "--to",
        "pdf",
        "--no-wait",
    ];
    let run = run(&addr, &[], &args).await;
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert_eq!(run.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("export e1 failed"), "{stderr}");
    assert!(stderr.contains("could not be rendered"), "{stderr}");
    assert!(stderr.contains("1 images missing (a1)"), "{stderr}");
    assert_eq!(seen.lines().len(), 1);
}

/// A download that fails after `succeeded` still shows the report.
#[tokio::test]
async fn a_failed_download_still_shows_the_report() {
    let (addr, seen) = spawn_stub(vec![
        (
            DOWNLOAD_PATH,
            vec![json_reply(
                404,
                &json!({"result": false, "error": {"code": 1, "text": "not found"}}),
            )],
        ),
        (
            START_PATH,
            vec![json_reply(
                200,
                &with_record(
                    &json!({"status": "succeeded", "output": {"expires": "2030-01-01"},
                    "report": {"orphaned_threads": [{"id": "c3", "reason": "ambiguous"}]}}),
                ),
            )],
        ),
    ])
    .await;
    let dir = scratch("download-fails");
    let out = dir.join("plan.pdf");
    let out_s = out.display().to_string();
    let run = run(&addr, &["--format", "json"], &start_args(&out_s, &[])).await;
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert_eq!(run.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("failed to download export e1"), "{stderr}");
    assert!(stderr.contains("c3: ambiguous"), "{stderr}");
    assert_eq!(stderr.matches("export report:").count(), 1, "{stderr}");
    assert!(run.stdout.is_empty());
    assert_eq!(seen.lines().len(), 2);
    assert!(listing(&dir).is_empty());
}

#[tokio::test]
async fn invalid_flags_are_refused_before_any_request() {
    let (addr, seen) =
        spawn_stub(vec![(START_PATH, vec![json_reply(202, &record("queued"))])]).await;
    let dir = scratch("flags");
    let out = dir.join("plan.pdf");
    let out_s = out.display().to_string();
    let tracked = run(&addr, &[], &start_args(&out_s, &["--tracked-changes"])).await;
    assert_ne!(tracked.status.code(), Some(0));
    assert!(
        String::from_utf8_lossy(&tracked.stderr).contains("docx or odt"),
        "{}",
        String::from_utf8_lossy(&tracked.stderr)
    );
    let both = run(&addr, &[], &start_args(&out_s, &["--save-copy"])).await;
    assert_ne!(both.status.code(), Some(0));
    let missing_dir = dir.join("nope").join("plan.pdf").display().to_string();
    let bad_out = run(&addr, &[], &start_args(&missing_dir, &[])).await;
    assert_ne!(bad_out.status.code(), Some(0));
    assert!(seen.all().is_empty(), "{:?}", seen.lines());
}

// ─── Binary runner ────────────────────────────────────────────────────────────

fn isolated_home() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fastio-export-wire-home-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create isolated home");
    dir
}

async fn run(addr: &str, global: &[&str], args: &[&str]) -> Output {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_fastio"))
        .arg("--api-base")
        .arg(format!("http://{addr}"))
        .arg("--token")
        .arg("test-token")
        .arg("--no-color")
        .args(global)
        .args(args)
        .env("HOME", isolated_home())
        .env_remove("FASTIO_TOKEN")
        .env_remove("FASTIO_API_KEY")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("HTTP_PROXY")
        .env_remove("http_proxy")
        .env_remove("HTTPS_PROXY")
        .env_remove("https_proxy")
        .env_remove("ALL_PROXY")
        .env_remove("all_proxy")
        .env("NO_PROXY", "*,127.0.0.1,::1,localhost")
        .output()
        .await
        .expect("run the fastio binary")
}
