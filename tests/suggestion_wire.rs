//! Wire-level and exit-code pins for `fastio suggestion` and the suggestion
//! routes.
//!
//! The accept route rewrites a document, so its sharpest guarantee is
//! negative: an attempt is put on the wire EXACTLY ONCE. A gateway error, or
//! a connection dropped after the request was written, must never cause a
//! second request — the first may already have applied the edit. The only
//! repeat is a single whole-accept retry when the server itself marks the
//! refusal safe to repeat. These tests count requests to prove both halves.
//!
//! The isolation measures for the binary runs follow
//! `tests/service_account_wire.rs`.

use std::process::Output;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fastio_cli::api::comment::{self, SuggestionChange};
use fastio_cli::client::ApiClient;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const WORKSPACE: &str = "1000000000000000001";
const NODE: &str = "nabcdefghijklmnopqrstuvwxyz12";
const SUGGESTION: &str = "cabcd-efghi-jklmn-opqrs-tuvwx-yz123";
const SUGGESTION_RAW: &str = "cabcdefghijklmnopqrstuvwxyz123";

/// One canned reply. `status == 0` drops the connection after reading the
/// request, without answering.
#[derive(Clone)]
struct Reply {
    status: u16,
    body: String,
}

fn reply(status: u16, body: &Value) -> Reply {
    Reply {
        status,
        body: body.to_string(),
    }
}

fn dropped() -> Reply {
    Reply {
        status: 0,
        body: String::new(),
    }
}

fn refusal(status: u16, params: &Value) -> Reply {
    reply(
        status,
        &json!({"result": false, "error": {"code": 170_001, "text": "refused", "params": params}}),
    )
}

/// What the stub saw.
#[derive(Clone, Default)]
struct Seen {
    count: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<(String, String)>>>,
}

impl Seen {
    fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    fn requests(&self) -> Vec<(String, String)> {
        self.requests.lock().map(|r| r.clone()).unwrap_or_default()
    }
}

/// Serve every connection: requests whose path starts with a route's prefix
/// get that route's replies IN ORDER (the last one repeats). Note reads
/// (`/workspace/`) and writes are routed separately so a create's note read
/// does not consume a write's reply.
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
            task_seen.count.fetch_add(1, Ordering::SeqCst);
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
                r.push((format!("{method} {path}"), body));
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
                // Drop the connection: the request was received, no reply.
                drop(sock);
                continue;
            }
            // A redirect points back at the same path, so following it would
            // show up as a second request.
            let location = if (300..400).contains(&chosen.status) {
                format!("Location: {path}\r\n")
            } else {
                String::new()
            };
            let header = format!(
                "HTTP/1.1 {} X\r\nContent-Type: application/json\r\n\
                 {location}Content-Length: {}\r\nConnection: close\r\n\r\n",
                chosen.status,
                chosen.body.len()
            );
            let _ = sock.write_all(header.as_bytes()).await;
            let _ = sock.write_all(chosen.body.as_bytes()).await;
            let _ = sock.flush().await;
        }
    });
    (addr, seen)
}

fn client(addr: &str) -> ApiClient {
    ApiClient::new(&format!("http://{addr}"), Some("tok".to_owned())).expect("client builds")
}

fn accepted() -> Value {
    json!({"result": true, "comment": {"id": SUGGESTION_RAW, "kind": "suggestion",
        "suggestion": {"state": "accepted", "applied_version_id": "v9"}}})
}

fn body_json(body: &str) -> Value {
    serde_json::from_str(body).expect("request body is JSON")
}

const ACCEPT_PATH: &str = "/comments/";

// ─── Accept: single attempt on the wire ───────────────────────────────────────

#[tokio::test]
async fn accept_sends_mode_headless_once_on_success() {
    let (addr, seen) = spawn_stub(vec![(ACCEPT_PATH, vec![reply(200, &accepted())])]).await;
    comment::accept_suggestion(&client(&addr), SUGGESTION, Duration::ZERO, |_| {})
        .await
        .expect("accept");
    let requests = seen.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].0,
        format!("POST /comments/{SUGGESTION_RAW}/suggestion/accept/")
    );
    assert_eq!(body_json(&requests[0].1), json!({"mode": "headless"}));
}

/// The gateway may have delivered the accept and lost the answer: never
/// re-send it.
#[tokio::test]
async fn accept_is_not_replayed_on_a_gateway_error() {
    for status in [502, 503, 504] {
        let (addr, seen) = spawn_stub(vec![(
            ACCEPT_PATH,
            vec![Reply {
                status,
                body: "<html>gateway</html>".to_owned(),
            }],
        )])
        .await;
        let err = comment::accept_suggestion(&client(&addr), SUGGESTION, Duration::ZERO, |_| {})
            .await
            .expect_err("a gateway error is surfaced");
        assert!(!comment::accept_is_retryable(&err));
        assert_eq!(seen.count(), 1, "HTTP {status} must not be replayed");
    }
}

#[tokio::test]
async fn accept_is_not_replayed_after_a_dropped_connection() {
    let (addr, seen) = spawn_stub(vec![(ACCEPT_PATH, vec![dropped()])]).await;
    let result =
        comment::accept_suggestion(&client(&addr), SUGGESTION, Duration::ZERO, |_| {}).await;
    assert!(result.is_err());
    assert_eq!(seen.count(), 1, "a dropped connection must not be replayed");
}

/// A 307/308 would make the default client re-send the POST body: the
/// redirect is refused, never followed, and the command fails.
#[tokio::test]
async fn accept_does_not_follow_a_redirect() {
    for status in [307, 308] {
        let (addr, seen) = spawn_stub(vec![(
            ACCEPT_PATH,
            vec![reply(status, &json!({"result": true}))],
        )])
        .await;
        let out = run(&addr, &[], &["suggestion", "accept", SUGGESTION]).await;
        assert_ne!(out.status.code(), Some(0), "HTTP {status} must fail");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("redirect"),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(seen.count(), 1, "HTTP {status} must not be followed");
    }
}

/// A suggestion create re-sent after a lost reply would duplicate it.
#[tokio::test]
async fn suggestion_create_is_not_replayed() {
    let reference = json!({"type": "document", "exact": "x", "prefix": "", "suffix": ""});
    for answer in [
        Reply {
            status: 503,
            body: "gateway".to_owned(),
        },
        Reply {
            status: 504,
            body: "gateway".to_owned(),
        },
        dropped(),
    ] {
        let status = answer.status;
        let (addr, seen) = spawn_stub(vec![("/comments/", vec![answer])]).await;
        let result = comment::add_suggestion(
            &client(&addr),
            &comment::AddSuggestionParams {
                entity_type: "workspace",
                entity_id: WORKSPACE,
                node_id: NODE,
                reference: &reference,
                change: SuggestionChange::Replace("y"),
                note: None,
            },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(seen.count(), 1, "status {status} must not be replayed");
    }
}

// ─── Accept: the one server-sanctioned retry ──────────────────────────────────

#[tokio::test]
async fn accept_retries_once_when_needs_editor_is_retryable() {
    let (addr, seen) = spawn_stub(vec![(
        ACCEPT_PATH,
        vec![
            refusal(
                409,
                &json!({"reason": "suggestion_needs_editor", "editor_reason": "room_busy", "retryable": true}),
            ),
            reply(200, &accepted()),
        ],
    )])
    .await;
    let mut told = 0;
    comment::accept_suggestion(&client(&addr), SUGGESTION, Duration::ZERO, |_| told += 1)
        .await
        .expect("second attempt succeeds");
    assert_eq!(told, 1);
    assert_eq!(seen.count(), 2);
}

#[tokio::test]
async fn accept_retries_once_on_apply_unknown_and_no_more() {
    let (addr, seen) = spawn_stub(vec![(
        ACCEPT_PATH,
        vec![refusal(
            503,
            &json!({"reason": "suggestion_apply_unknown", "retryable": true}),
        )],
    )])
    .await;
    let err = comment::accept_suggestion(&client(&addr), SUGGESTION, Duration::ZERO, |_| {})
        .await
        .expect_err("still unknown");
    assert_eq!(
        seen.count(),
        2,
        "exactly one retry, then the outcome is reported"
    );
    assert_eq!(
        err.suggestion(),
        Some(fastio_cli::error::HINT_SUGGESTION_APPLY_UNKNOWN)
    );
}

#[tokio::test]
async fn accept_does_not_retry_unretryable_refusals() {
    for params in [
        json!({"reason": "suggestion_needs_editor", "editor_reason": "viewers_present", "retryable": false}),
        json!({"reason": "suggestion_stale"}),
        json!({"reason": "suggestion_not_pending"}),
    ] {
        let (addr, seen) = spawn_stub(vec![(ACCEPT_PATH, vec![refusal(409, &params)])]).await;
        let _ =
            comment::accept_suggestion(&client(&addr), SUGGESTION, Duration::ZERO, |_| {}).await;
        assert_eq!(seen.count(), 1, "{params} must not be retried");
    }
    let (addr, seen) = spawn_stub(vec![(
        ACCEPT_PATH,
        vec![refusal(
            503,
            &json!({"reason": "suggestion_apply_unavailable", "retryable": true, "retry_after": 5}),
        )],
    )])
    .await;
    let _ = comment::accept_suggestion(&client(&addr), SUGGESTION, Duration::ZERO, |_| {}).await;
    assert_eq!(seen.count(), 1, "apply_unavailable is not auto-retried");
}

// ─── Accept: exit codes through the real binary ───────────────────────────────

#[tokio::test]
async fn accept_exit_codes() {
    let cases = [
        (
            refusal(
                409,
                &json!({"reason": "suggestion_needs_editor", "editor_reason": "viewers_present", "retryable": false}),
            ),
            5,
            "Open the note",
        ),
        (
            refusal(409, &json!({"reason": "suggestion_stale"})),
            1,
            "has changed",
        ),
        (
            refusal(409, &json!({"reason": "suggestion_not_pending"})),
            1,
            "no longer pending",
        ),
        (
            refusal(
                503,
                &json!({"reason": "suggestion_apply_unavailable", "retryable": true}),
            ),
            1,
            "suggestion's state",
        ),
        (
            refusal(403, &json!({"reason": "suggestion_accept_forbidden"})),
            1,
            "edit rights",
        ),
    ];
    for (answer, code, hint) in cases {
        let (addr, seen) = spawn_stub(vec![(ACCEPT_PATH, vec![answer])]).await;
        let out = run(&addr, &[], &["suggestion", "accept", SUGGESTION]).await;
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(code), "{stderr}");
        assert!(stderr.contains(hint), "expected hint `{hint}` in: {stderr}");
        assert_eq!(seen.count(), 1);
    }

    let (addr, _seen) = spawn_stub(vec![(ACCEPT_PATH, vec![reply(200, &accepted())])]).await;
    let out = run(
        &addr,
        &["--format", "json"],
        &["suggestion", "accept", SUGGESTION],
    )
    .await;
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ─── Create / edit / withdraw / reject / list ─────────────────────────────────

#[tokio::test]
async fn cli_create_sends_kind_suggestion_with_anchor_and_rationale() {
    let (addr, seen) = spawn_stub(vec![
        (
            "/workspace/",
            vec![reply(
                200,
                &json!({"result": true, "content": "Ship it on Friday afternoon.", "note": {}}),
            )],
        ),
        (
            "/comments/",
            vec![reply(
                200,
                &json!({"result": true, "comment": {"id": SUGGESTION_RAW, "kind": "suggestion"}}),
            )],
        ),
    ])
    .await;
    let out = run(
        &addr,
        &["--format", "json"],
        &[
            "suggestion",
            "create",
            NODE,
            "--entity-type",
            "workspace",
            "--entity-id",
            WORKSPACE,
            "--quote",
            "Friday",
            "--replacement",
            "Thursday",
            "--note",
            "Friday is a holiday",
        ],
    )
    .await;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let requests = seen.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].0,
        format!("POST /comments/workspace/{WORKSPACE}/{NODE}/")
    );
    assert_eq!(
        body_json(&requests[1].1),
        json!({
            "body": "Friday is a holiday",
            "kind": "suggestion",
            "reference": {"type": "document", "exact": "Friday", "prefix": "Ship it on ", "suffix": " afternoon."},
            "suggestion": {"op": "replace", "replacement": "Thursday"}
        })
    );
}

#[tokio::test]
async fn cli_create_delete_sends_an_empty_replacement_and_body() {
    let (addr, seen) = spawn_stub(vec![
        (
            "/workspace/",
            vec![reply(
                200,
                &json!({"result": true, "content": "keep this drop that", "note": {}}),
            )],
        ),
        (
            "/comments/",
            vec![reply(
                200,
                &json!({"result": true, "comment": {"kind": "suggestion"}}),
            )],
        ),
    ])
    .await;
    let out = run(
        &addr,
        &["--format", "json"],
        &[
            "suggestion",
            "create",
            NODE,
            "--entity-type",
            "workspace",
            "--entity-id",
            WORKSPACE,
            "--quote",
            "drop that",
            "--delete",
        ],
    )
    .await;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body = body_json(&seen.requests()[1].1);
    assert_eq!(body["body"], json!(""));
    assert_eq!(
        body["suggestion"],
        json!({"op": "delete", "replacement": ""})
    );

    // --replacement and --delete together, or neither: refused by clap.
    let both = run(
        &addr,
        &[],
        &[
            "suggestion",
            "create",
            NODE,
            "--entity-type",
            "workspace",
            "--entity-id",
            WORKSPACE,
            "--quote",
            "x",
            "--delete",
            "--replacement",
            "y",
        ],
    )
    .await;
    assert_eq!(both.status.code(), Some(2));
    let neither = run(
        &addr,
        &[],
        &[
            "suggestion",
            "create",
            NODE,
            "--entity-type",
            "workspace",
            "--entity-id",
            WORKSPACE,
            "--quote",
            "x",
        ],
    )
    .await;
    assert_eq!(neither.status.code(), Some(2));
    assert_eq!(seen.count(), 2, "refused invocations send nothing");
}

/// A server that ignores `kind` creates a plain comment: say so.
#[tokio::test]
async fn cli_create_warns_when_the_server_ignored_kind() {
    let (addr, _seen) = spawn_stub(vec![
        (
            "/workspace/",
            vec![reply(
                200,
                &json!({"result": true, "content": "abc", "note": {}}),
            )],
        ),
        (
            "/comments/",
            vec![reply(
                200,
                &json!({"result": true, "comment": {"id": "c1"}}),
            )],
        ),
    ])
    .await;
    let out = run(
        &addr,
        &["--format", "json"],
        &[
            "suggestion",
            "create",
            NODE,
            "--entity-type",
            "workspace",
            "--entity-id",
            WORKSPACE,
            "--quote",
            "abc",
            "--replacement",
            "xyz",
        ],
    )
    .await;
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("plain comment"));
}

#[tokio::test]
async fn edit_withdraw_reject_wire() {
    let ok = reply(
        200,
        &json!({"result": true, "comment": {"id": SUGGESTION_RAW}}),
    );
    let (addr, seen) = spawn_stub(vec![(ACCEPT_PATH, vec![ok])]).await;
    let c = client(&addr);
    comment::edit_suggestion(
        &c,
        SUGGESTION,
        Some(SuggestionChange::Replace("new")),
        Some("why"),
    )
    .await
    .expect("edit");
    comment::edit_suggestion(&c, SUGGESTION, Some(SuggestionChange::Delete), None)
        .await
        .expect("edit to delete");
    comment::withdraw_suggestion(&c, SUGGESTION)
        .await
        .expect("withdraw");
    comment::reject_suggestion(&c, SUGGESTION)
        .await
        .expect("reject");
    let requests = seen.requests();
    let expect = [
        (
            "update",
            json!({"suggestion": {"op": "replace", "replacement": "new"}, "body": "why"}),
        ),
        (
            "update",
            json!({"suggestion": {"op": "delete", "replacement": ""}}),
        ),
        ("suggestion/withdraw", json!({})),
        ("suggestion/reject", json!({})),
    ];
    assert_eq!(requests.len(), expect.len());
    for ((line, body), (action, want)) in requests.iter().zip(expect) {
        assert_eq!(line, &format!("POST /comments/{SUGGESTION_RAW}/{action}/"));
        assert_eq!(body_json(body), want);
    }
}

#[tokio::test]
async fn withdraw_and_reject_are_not_replayed_on_a_gateway_error() {
    let (addr, seen) = spawn_stub(vec![(
        ACCEPT_PATH,
        vec![Reply {
            status: 502,
            body: "gateway".to_owned(),
        }],
    )])
    .await;
    let c = client(&addr);
    assert!(comment::withdraw_suggestion(&c, SUGGESTION).await.is_err());
    assert!(comment::reject_suggestion(&c, SUGGESTION).await.is_err());
    assert_eq!(seen.count(), 2, "one request each");
}

#[tokio::test]
async fn cli_list_sends_kind_suggestion() {
    let (addr, seen) = spawn_stub(vec![(
        "/comments/",
        vec![reply(200, &json!({"result": true, "comments": []}))],
    )])
    .await;
    let node = run(
        &addr,
        &["--format", "json"],
        &[
            "suggestion",
            "list",
            NODE,
            "--entity-type",
            "workspace",
            "--entity-id",
            WORKSPACE,
            "--status",
            "open",
            "--for-me",
        ],
    )
    .await;
    assert!(
        node.status.success(),
        "{}",
        String::from_utf8_lossy(&node.stderr)
    );
    let all = run(
        &addr,
        &["--format", "json"],
        &[
            "suggestion",
            "list",
            "--entity-type",
            "workspace",
            "--entity-id",
            WORKSPACE,
        ],
    )
    .await;
    assert!(
        all.status.success(),
        "{}",
        String::from_utf8_lossy(&all.stderr)
    );
    let requests = seen.requests();
    assert!(
        requests[0]
            .0
            .starts_with(&format!("GET /comments/workspace/{WORKSPACE}/{NODE}/?"))
    );
    for pair in ["kind=suggestion", "status=open", "for_me=true"] {
        assert!(requests[0].0.contains(pair), "{pair}: {}", requests[0].0);
    }
    assert!(
        requests[1]
            .0
            .starts_with(&format!("GET /comments/workspace/{WORKSPACE}/?"))
    );
    assert!(requests[1].0.contains("kind=suggestion"));
}

// ─── Binary runner ────────────────────────────────────────────────────────────

fn isolated_home() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("fastio-suggestion-wire-{}", std::process::id()));
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
