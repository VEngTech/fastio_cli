//! Wire-level pins for the comment thread routes: list filters, resolve /
//! reopen / assign / unassign, reply-and-reopen, and quote anchors.
//!
//! Each test drives the real request builders (or the real `fastio` binary)
//! against a loopback stub and asserts the METHOD, PATH and JSON BODY that
//! reached it — the only place a misspelt key or a path one segment off is
//! visible. The stub answers per route so a quote anchor's note read and the
//! create that follows it can be observed in order.
//!
//! The isolation measures for the binary runs (throwaway `HOME`, removed proxy
//! variables, loopback `NO_PROXY`) follow `tests/service_account_wire.rs`.

use std::process::Output;
use std::sync::{Arc, Mutex};

use fastio_cli::api::comment::{self, Assignee, CommentListFilters, ListCommentsParams};
use fastio_cli::client::ApiClient;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

const WORKSPACE: &str = "1000000000000000001";
const NODE: &str = "nabcd-efghi-jklmn-opqrs-tuvwx-yz12";
const NODE_RAW: &str = "nabcdefghijklmnopqrstuvwxyz12";
const COMMENT: &str = "cabcd-efghi-jklmn-opqrs-tuvwx-yz123";
const COMMENT_RAW: &str = "cabcdefghijklmnopqrstuvwxyz123";

/// One canned answer: requests whose path starts with `path_prefix` get
/// `status` + `body`.
struct Route {
    path_prefix: String,
    status: u16,
    body: String,
}

/// One captured request: `METHOD /path?query` plus the raw body.
type Captured = Arc<Mutex<Vec<(String, String)>>>;

/// Serve every connection on a fresh loopback port from `routes` (first
/// matching prefix wins; no match is a 404) and capture each request.
async fn spawn_stub(routes: Vec<Route>) -> (String, Captured) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr").to_string();
    let captured: Captured = Arc::default();
    let sink = Arc::clone(&captured);
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
            if let Ok(mut c) = sink.lock() {
                c.push((format!("{method} {path}"), body));
            }
            let (status, reply) = routes
                .iter()
                .find(|r| path.starts_with(&r.path_prefix))
                .map_or(
                    (
                        404,
                        r#"{"result":false,"error":{"code":9992,"text":"no route"}}"#.to_owned(),
                    ),
                    |r| (r.status, r.body.clone()),
                );
            let header = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n",
                reply.len()
            );
            let _ = sock.write_all(header.as_bytes()).await;
            let _ = sock.write_all(reply.as_bytes()).await;
            let _ = sock.flush().await;
        }
    });
    (addr, captured)
}

fn ok_route(path_prefix: &str, body: &Value) -> Route {
    Route {
        path_prefix: path_prefix.to_owned(),
        status: 200,
        body: body.to_string(),
    }
}

fn requests(captured: &Captured) -> Vec<(String, String)> {
    captured.lock().map(|c| c.clone()).unwrap_or_default()
}

fn body_json(body: &str) -> Value {
    serde_json::from_str(body).expect("request body is JSON")
}

fn client(addr: &str) -> ApiClient {
    ApiClient::new(&format!("http://{addr}"), Some("tok".to_owned())).expect("client builds")
}

fn comment_ok() -> Value {
    json!({"result": true, "comment": {"id": COMMENT_RAW, "state_version": 2}})
}

// ─── List ─────────────────────────────────────────────────────────────────────

/// A listing with no new filters must send exactly what it always did.
#[tokio::test]
async fn list_without_filters_is_unchanged() {
    let (addr, captured) = spawn_stub(vec![ok_route(
        "/",
        &json!({"result": true, "comments": []}),
    )])
    .await;
    let c = client(&addr);
    let mut params = ListCommentsParams {
        entity_type: "workspace",
        entity_id: WORKSPACE,
        node_id: NODE_RAW,
        sort: None,
        limit: None,
        offset: None,
        filters: CommentListFilters::default(),
    };
    comment::list_comments(&c, &params).await.expect("list");
    params.limit = Some(10);
    comment::list_comments(&c, &params).await.expect("list");
    comment::list_all_comments(
        &c,
        "workspace",
        WORKSPACE,
        None,
        None,
        None,
        &CommentListFilters::default(),
    )
    .await
    .expect("list-all");

    let seen = requests(&captured);
    assert_eq!(
        seen[0].0,
        format!("GET /comments/workspace/{WORKSPACE}/{NODE_RAW}/")
    );
    assert_eq!(
        seen[1].0,
        format!("GET /comments/workspace/{WORKSPACE}/{NODE_RAW}/?limit=10")
    );
    assert_eq!(seen[2].0, format!("GET /comments/workspace/{WORKSPACE}/"));
}

/// A dashed (display-form) node id goes dash-free into every node path:
/// list, create and reply. File Share paths refuse the dashed form.
#[tokio::test]
async fn dashed_node_ids_are_dash_free_in_every_node_path() {
    let (addr, captured) = spawn_stub(vec![ok_route("/", &comment_ok())]).await;
    let c = client(&addr);
    comment::list_comments(
        &c,
        &ListCommentsParams {
            entity_type: "fileshare",
            entity_id: WORKSPACE,
            node_id: NODE,
            sort: None,
            limit: None,
            offset: None,
            filters: CommentListFilters::default(),
        },
    )
    .await
    .expect("list");
    for parent_id in [None, Some(COMMENT)] {
        comment::add_comment(
            &c,
            &comment::AddCommentParams {
                entity_type: "fileshare",
                entity_id: WORKSPACE,
                node_id: NODE,
                body: "hello",
                parent_id,
                reference: None,
                properties: None,
                target_id: None,
                target_ids: None,
                reopen: false,
            },
        )
        .await
        .expect("create / reply");
    }
    let seen = requests(&captured);
    let path = format!("/comments/fileshare/{WORKSPACE}/{NODE_RAW}/");
    assert_eq!(seen[0].0, format!("GET {path}"));
    assert_eq!(seen[1].0, format!("POST {path}"));
    assert_eq!(seen[2].0, format!("POST {path}"));
    assert_eq!(body_json(&seen[2].1)["parent_id"], json!(COMMENT));
}

#[tokio::test]
async fn list_filters_reach_the_query() {
    let (addr, captured) = spawn_stub(vec![ok_route(
        "/",
        &json!({"result": true, "comments": []}),
    )])
    .await;
    let c = client(&addr);
    let filters = CommentListFilters {
        status: Some("open"),
        for_me: Some("agents"),
        kind: Some("all"),
        include_suggestions: true,
        threads: true,
    };
    comment::list_comments(
        &c,
        &ListCommentsParams {
            entity_type: "workspace",
            entity_id: WORKSPACE,
            node_id: NODE,
            sort: None,
            limit: None,
            offset: None,
            filters,
        },
    )
    .await
    .expect("list");
    comment::list_all_comments(&c, "share", WORKSPACE, None, None, None, &filters)
        .await
        .expect("list-all");
    for (line, _) in requests(&captured) {
        for pair in [
            "status=open",
            "for_me=agents",
            "kind=all",
            "include_suggestions=true",
            "group=threads",
        ] {
            assert!(line.contains(pair), "{pair} missing from {line}");
        }
    }
}

/// `--threads` with table output moves `counts`/`limits` to stderr; JSON
/// keeps them inline.
#[tokio::test]
async fn list_counts_summary_goes_to_stderr_for_table_only() {
    let page = json!({
        "result": true,
        "threads": [{"id": COMMENT_RAW, "body": "hello"}],
        "counts": {"all": 1, "open": 1},
        "limits": {"root_threads_used": 1, "root_threads_max": 100},
        "limit": 50, "offset": 0, "total": 1
    });
    let (addr, captured) = spawn_stub(vec![ok_route("/", &page)]).await;
    let args = [
        "comment",
        "list",
        NODE,
        "--entity-type",
        "workspace",
        "--entity-id",
        WORKSPACE,
        "--threads",
    ];
    let table = run(&addr, &["--format", "table"], &args).await;
    assert!(
        table.status.success(),
        "{}",
        String::from_utf8_lossy(&table.stderr)
    );
    let stderr = String::from_utf8_lossy(&table.stderr);
    assert!(
        stderr.contains("open=1") && stderr.contains("threads: 1 of 100"),
        "{stderr}"
    );
    let stdout = String::from_utf8_lossy(&table.stdout);
    assert!(!stdout.contains("root_threads_max"), "{stdout}");

    let as_json = run(&addr, &["--format", "json"], &args).await;
    assert!(as_json.status.success());
    let parsed: Value = serde_json::from_slice(&as_json.stdout).expect("json output");
    assert_eq!(parsed["counts"]["open"], json!(1));
    assert_eq!(parsed["limits"]["root_threads_max"], json!(100));

    assert!(
        requests(&captured)
            .iter()
            .all(|(line, _)| line.contains("group=threads")),
        "--threads must send group=threads"
    );
}

// ─── Thread state ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn resolve_reopen_assign_unassign_wire() {
    let (addr, captured) = spawn_stub(vec![ok_route("/", &comment_ok())]).await;
    let c = client(&addr);
    comment::resolve_comment(&c, COMMENT, None)
        .await
        .expect("resolve");
    comment::resolve_comment(&c, COMMENT, Some(4))
        .await
        .expect("resolve cas");
    comment::reopen_comment(&c, COMMENT, Some(5))
        .await
        .expect("reopen");
    comment::assign_comment(&c, COMMENT, &Assignee::Caller)
        .await
        .expect("assign self");
    comment::assign_comment(&c, COMMENT, &Assignee::Ripley)
        .await
        .expect("assign ripley");
    comment::assign_comment(
        &c,
        COMMENT,
        &Assignee::User {
            user_id: "1000000000000000002",
            agent: None,
        },
    )
    .await
    .expect("assign user");
    comment::unassign_comment(&c, COMMENT)
        .await
        .expect("unassign");

    let seen = requests(&captured);
    let expect = [
        ("resolve", json!({})),
        ("resolve", json!({"if_state_version": 4})),
        ("reopen", json!({"if_state_version": 5})),
        ("assign", json!({"assignee": "self"})),
        ("assign", json!({"assignee": "ripley"})),
        ("assign", json!({"assignee_user_id": "1000000000000000002"})),
        ("assign", json!({"clear": true})),
    ];
    assert_eq!(seen.len(), expect.len());
    for ((line, body), (action, want)) in seen.iter().zip(expect) {
        assert_eq!(line, &format!("POST /comments/{COMMENT_RAW}/{action}/"));
        assert_eq!(body_json(body), want, "{line}");
    }
}

#[tokio::test]
async fn cli_assign_with_agent_qualifier() {
    let (addr, captured) = spawn_stub(vec![ok_route("/", &comment_ok())]).await;
    let out = run(
        &addr,
        &["--format", "json"],
        &[
            "comment",
            "assign",
            COMMENT,
            "--user-id",
            "1000000000000000002",
            "--agent-name",
            "helper",
            "--credential-type",
            "api_key",
        ],
    )
    .await;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let seen = requests(&captured);
    assert_eq!(seen[0].0, format!("POST /comments/{COMMENT_RAW}/assign/"));
    assert_eq!(
        body_json(&seen[0].1),
        json!({
            "assignee_user_id": "1000000000000000002",
            "assignee_agent": {"agent_name": "helper", "credential_type": "api_key"}
        })
    );

    // Exactly one assignee form: clap refuses none and refuses two, before
    // any request.
    let none = run(&addr, &[], &["comment", "assign", COMMENT]).await;
    assert_eq!(none.status.code(), Some(2));
    let two = run(
        &addr,
        &[],
        &["comment", "assign", COMMENT, "--self", "--ripley"],
    )
    .await;
    assert_eq!(two.status.code(), Some(2));
    assert_eq!(requests(&captured).len(), 1);
}

#[tokio::test]
async fn cli_reply_reopen_sends_reopen_on_the_create() {
    let (addr, captured) = spawn_stub(vec![ok_route("/", &comment_ok())]).await;
    let out = run(
        &addr,
        &["--format", "json"],
        &[
            "comment",
            "reply",
            COMMENT,
            "Fixed in the latest draft.",
            "--node-id",
            NODE,
            "--entity-type",
            "workspace",
            "--entity-id",
            WORKSPACE,
            "--reopen",
        ],
    )
    .await;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let seen = requests(&captured);
    assert_eq!(
        seen[0].0,
        format!("POST /comments/workspace/{WORKSPACE}/{NODE_RAW}/")
    );
    assert_eq!(
        body_json(&seen[0].1),
        json!({"body": "Fixed in the latest draft.", "parent_id": COMMENT, "reopen": true})
    );
}

/// A reasonless 404 from a new route says the server may not support it yet.
#[tokio::test]
async fn new_route_404_hints_not_supported_yet() {
    let (addr, _captured) = spawn_stub(vec![]).await;
    let out = run(&addr, &[], &["comment", "resolve", COMMENT]).await;
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("does not support this yet"), "{stderr}");
}

// ─── Quote anchors ────────────────────────────────────────────────────────────

fn note_route(content: &str) -> Route {
    ok_route(
        &format!("/workspace/{WORKSPACE}/storage/"),
        &json!({"result": true, "content": content, "note": {"id": NODE_RAW}}),
    )
}

/// `--quote` reads the note, derives the safe-window context, and sends a
/// document reference with no offsets.
#[tokio::test]
async fn cli_quote_derives_context_from_the_note() {
    let (addr, captured) = spawn_stub(vec![
        note_route("Intro.\nWe use **bold** for the quarterly plan here.\n"),
        ok_route("/comments/", &comment_ok()),
    ])
    .await;
    let out = run(
        &addr,
        &["--format", "json"],
        &[
            "comment",
            "create",
            NODE,
            "Is this still current?",
            "--entity-type",
            "workspace",
            "--entity-id",
            WORKSPACE,
            "--quote",
            "quarterly plan",
        ],
    )
    .await;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let seen = requests(&captured);
    assert_eq!(seen.len(), 2);
    assert!(
        seen[0]
            .0
            .starts_with(&format!("GET /workspace/{WORKSPACE}/storage/")),
        "{}",
        seen[0].0
    );
    assert!(seen[0].0.contains("/readnote/"), "{}", seen[0].0);
    assert_eq!(
        seen[1].0,
        format!("POST /comments/workspace/{WORKSPACE}/{NODE_RAW}/")
    );
    assert_eq!(
        body_json(&seen[1].1),
        json!({
            "body": "Is this still current?",
            "reference": {
                "type": "document",
                "exact": "quarterly plan",
                "prefix": " for the ",
                "suffix": " here."
            }
        })
    );
}

/// Explicit context is still checked: a pair that matches twice is refused
/// before any create is sent.
#[tokio::test]
async fn cli_quote_ambiguous_is_refused_before_create() {
    let (addr, captured) = spawn_stub(vec![
        note_route("the cat sat\nthe cat ran"),
        ok_route("/comments/", &comment_ok()),
    ])
    .await;
    let out = run(
        &addr,
        &[],
        &[
            "comment",
            "create",
            NODE,
            "Which one?",
            "--entity-type",
            "workspace",
            "--entity-id",
            WORKSPACE,
            "--quote",
            "cat",
            "--prefix",
            "the ",
        ],
    )
    .await;
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("2 times"), "{stderr}");
    assert!(
        requests(&captured)
            .iter()
            .all(|(line, _)| !line.starts_with("POST")),
        "no create may be sent for an ambiguous anchor"
    );
}

/// A quote that is not in the stored text is still sent, with a warning.
#[tokio::test]
async fn cli_quote_not_found_warns_and_sends() {
    let (addr, captured) = spawn_stub(vec![
        note_route("Older stored text."),
        ok_route("/comments/", &comment_ok()),
    ])
    .await;
    let out = run(
        &addr,
        &["--format", "json"],
        &[
            "comment",
            "create",
            NODE,
            "New paragraph?",
            "--entity-type",
            "workspace",
            "--entity-id",
            WORKSPACE,
            "--quote",
            "freshly typed words",
        ],
    )
    .await;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("not found"));
    let seen = requests(&captured);
    assert_eq!(
        body_json(&seen[1].1)["reference"],
        json!({"type": "document", "exact": "freshly typed words", "prefix": "", "suffix": ""})
    );
}

/// `--quote` and `--reference` are mutually exclusive (clap, no request).
#[tokio::test]
async fn cli_quote_conflicts_with_reference() {
    let (addr, captured) = spawn_stub(vec![]).await;
    let out = run(
        &addr,
        &[],
        &[
            "comment",
            "create",
            NODE,
            "x",
            "--entity-type",
            "workspace",
            "--entity-id",
            WORKSPACE,
            "--quote",
            "a",
            "--reference",
            "{\"type\":\"document\"}",
        ],
    )
    .await;
    assert_eq!(out.status.code(), Some(2));
    assert!(requests(&captured).is_empty());
}

// ─── View ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn view_include_suggestions_sends_the_flag_and_prints_them() {
    let (addr, captured) = spawn_stub(vec![ok_route(
        "/",
        &json!({
            "result": true,
            "content": "Body",
            "note": {"id": NODE_RAW},
            "suggestions": [{"id": COMMENT_RAW, "op": "replace", "exact": "Body", "replacement": "Text"}]
        }),
    )])
    .await;
    let out = run(
        &addr,
        &["--format", "json"],
        &[
            "view",
            WORKSPACE,
            NODE,
            "--include-suggestions",
            "--version",
            "v1",
        ],
    )
    .await;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let parsed: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(parsed["content"], json!("Body"));
    assert_eq!(parsed["suggestions"][0]["replacement"], json!("Text"));
    let line = &requests(&captured)[0].0;
    assert!(line.contains("include_suggestions=true"), "{line}");
    assert!(line.contains("version_id=v1"), "{line}");

    let raw = run(
        &addr,
        &[],
        &["view", WORKSPACE, NODE, "--include-suggestions", "--raw"],
    )
    .await;
    assert_eq!(raw.status.code(), Some(2), "--raw cannot combine with it");
}

/// Every format shows BOTH the note text and the suggestions: table and CSV
/// would otherwise flatten to the suggestions array and drop the text.
#[tokio::test]
async fn view_include_suggestions_keeps_content_in_every_format() {
    let (addr, _captured) = spawn_stub(vec![ok_route(
        "/",
        &json!({
            "result": true,
            "content": "Quarterly note body",
            "note": {"id": NODE_RAW},
            "suggestions": [{"id": COMMENT_RAW, "op": "replace", "exact": "body", "replacement": "Replacementzz"}]
        }),
    )])
    .await;
    for format in ["json", "markdown", "table"] {
        let out = run(
            &addr,
            &["--format", format],
            &["view", WORKSPACE, NODE, "--include-suggestions"],
        )
        .await;
        assert!(
            out.status.success(),
            "{format}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("Quarterly note body"), "{format}: {stdout}");
        assert!(stdout.contains("Replacementzz"), "{format}: {stdout}");
    }
}

/// A note with no trailing newline, and suggestion text that needs CSV
/// quoting (commas, quotes, a newline).
fn tricky_note_page() -> Value {
    json!({
        "result": true,
        "content": "Note line one\nNote line two",
        "note": {"id": NODE_RAW},
        "suggestions": [
            {"id": "c1", "op": "replace", "exact": "one", "replacement": "a, \"quoted\"\nvalue"},
            {"id": "c2", "op": "delete", "exact": "two", "replacement": ""}
        ]
    })
}

/// Table: the note, a newline even though the note has none, a blank line,
/// then the suggestions table.
#[tokio::test]
async fn view_include_suggestions_table_separates_note_and_table() {
    let (addr, _captured) = spawn_stub(vec![ok_route("/", &tricky_note_page())]).await;
    let out = run(
        &addr,
        &["--format", "table"],
        &["view", WORKSPACE, NODE, "--include-suggestions"],
    )
    .await;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let (note, table) = stdout
        .split_once("\n\n")
        .expect("a blank line separates the note from the table");
    assert_eq!(note, "Note line one\nNote line two");
    assert!(table.contains("c1") && table.contains("c2"), "{table}");
}

/// CSV: only the suggestion rows, as a valid CSV document; the note text is
/// left out (with a note on stderr).
#[tokio::test]
async fn view_include_suggestions_csv_is_valid_and_rows_only() {
    let (addr, _captured) = spawn_stub(vec![ok_route("/", &tricky_note_page())]).await;
    let out = run(
        &addr,
        &["--format", "csv"],
        &["view", WORKSPACE, NODE, "--include-suggestions"],
    )
    .await;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("omitted from CSV"));
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        !stdout.contains("Note line"),
        "no note text in CSV: {stdout}"
    );

    let mut reader = csv::Reader::from_reader(out.stdout.as_slice());
    let mut headers: Vec<String> = reader
        .headers()
        .expect("CSV header")
        .iter()
        .map(str::to_owned)
        .collect();
    headers.sort();
    assert_eq!(headers, ["exact", "id", "op", "replacement"]);
    let header_row = reader.headers().expect("CSV header").clone();
    let column = |name: &str| header_row.iter().position(|h| h == name).expect("column");
    let rows: Vec<csv::StringRecord> = reader
        .records()
        .collect::<Result<_, _>>()
        .expect("every row is valid CSV");
    assert_eq!(rows.len(), 2);
    assert_eq!(&rows[0][column("replacement")], "a, \"quoted\"\nvalue");
    assert_eq!(&rows[1][column("id")], "c2");

    // `--fields` narrows the suggestion rows, as on other listings.
    let narrowed = run(
        &addr,
        &["--format", "csv", "--fields", "id"],
        &["view", WORKSPACE, NODE, "--include-suggestions"],
    )
    .await;
    assert!(narrowed.status.success());
    let mut reader = csv::Reader::from_reader(narrowed.stdout.as_slice());
    assert_eq!(
        reader.headers().expect("header").iter().collect::<Vec<_>>(),
        ["id"]
    );
}

// ─── Binary runner ────────────────────────────────────────────────────────────

fn isolated_home() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("fastio-comments-v2-wire-{}", std::process::id()));
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
