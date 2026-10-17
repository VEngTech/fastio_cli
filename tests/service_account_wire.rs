//! End-to-end locks for `fastio service-account` (alias `sa`).
//!
//! These drive the real `fastio` binary against a loopback stub, because the
//! guarantees live in the WIRING, not in any one helper:
//!
//! - `create` and `key create` print the one-time secret exactly once in every
//!   `--format` (the raw `{service_account, key}` shape loses the key in the
//!   table / CSV flattening);
//! - `--quiet` and `--fields` are refused on those two commands before a
//!   single request is sent;
//! - a mint is never replayed after a gateway error;
//! - `list` keeps the plan limit and `info` keeps the account in table / CSV.
//!
//! The isolation measures (throwaway `HOME`, removed proxy variables, explicit
//! loopback `NO_PROXY`) follow `tests/metadata_details_render.rs`; see that
//! file for why each one is needed.

use std::process::Output;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

const WORKSPACE: &str = "1000000000000000001";
const SA_ID: &str = "5555555555555555555";
const KEY_ID: &str = "kAbCdEfGhIjKlMnOpQrS";
/// A synthetic secret in the documented shape: `fio_sa_` + 43 alphanumerics.
const SECRET: &str = "fio_sa_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8S9t0U1v";
const ALL_FORMATS: [&str; 4] = ["json", "markdown", "table", "csv"];

/// What the stub saw: one entry per request, `METHOD PATH` plus the body.
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

/// Serve `status` + `body` to EVERY connection on a fresh loopback port, and
/// count the requests. Serving every connection (not just the first) is what
/// lets a test observe a replay.
async fn spawn_stub(status: u16, body: String) -> (String, Seen) {
    spawn_stub_with_location(status, body, None).await
}

/// [`spawn_stub`] that also sends a `Location` header (for redirect tests).
async fn spawn_stub_with_location(
    status: u16,
    body: String,
    location: Option<String>,
) -> (String, Seen) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr").to_string();
    let seen = Seen::default();
    let task_seen = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            task_seen.count.fetch_add(1, Ordering::SeqCst);
            // Read the headers, then exactly Content-Length body bytes.
            let mut raw = Vec::new();
            let mut buf = [0u8; 4096];
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
            let request_line = head.lines().next().unwrap_or_default();
            let mut parts = request_line.split_whitespace();
            let method_path = format!(
                "{} {}",
                parts.next().unwrap_or_default(),
                parts.next().unwrap_or_default()
            );
            let req_body = String::from_utf8_lossy(&raw[header_end..]).to_string();
            if let Ok(mut r) = task_seen.requests.lock() {
                r.push((method_path, req_body));
            }

            let reason = if status == 200 { "OK" } else { "Bad Gateway" };
            let content_type = if status == 200 {
                "application/json"
            } else {
                "text/html"
            };
            let location_header = location
                .as_deref()
                .map(|l| format!("Location: {l}\r\n"))
                .unwrap_or_default();
            let header = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\n\
                 {location_header}Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = sock.write_all(header.as_bytes()).await;
            let _ = sock.write_all(body.as_bytes()).await;
            let _ = sock.flush().await;
        }
    });
    (addr, seen)
}

/// A throwaway `HOME` so the child never reads the developer's real profile.
fn isolated_home() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("fastio-sa-wire-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create isolated home");
    dir
}

/// Run the real binary against `addr` with `args` (after the global flags).
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

fn stdout_of(output: &Output, what: &str) -> String {
    assert!(
        output.status.success(),
        "{what} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).expect("stdout is utf-8")
}

fn account() -> Value {
    json!({
        "id": SA_ID,
        "workspace_id": WORKSPACE,
        "name": "ci-bot",
        "role": "editor",
        "status": "active",
        "created": "2026-10-16 12:00:00 UTC",
        "updated": "2026-10-16 12:00:00 UTC",
        "created_by": {"id": "1111111111111111111", "name": "Admin"},
        "keys_active": 1,
        "next_key_expiry": "2027-01-14 12:00:00 UTC",
        "last_used": null
    })
}

fn key(id: &str) -> Value {
    json!({
        "id": id,
        "hint": "fio_sa_…U1v",
        "memo": "deploys",
        "created": "2026-10-16 12:00:00 UTC",
        "created_by": {"id": "1111111111111111111", "name": "Admin"},
        "expires": "2027-01-14 12:00:00 UTC",
        "expired": false,
        "last_used": null
    })
}

fn key_with_secret() -> Value {
    let mut k = key(KEY_ID);
    k["secret"] = json!(SECRET);
    k
}

/// The create body at the envelope's top level (no `response` wrapper).
fn create_body() -> String {
    json!({"result": "yes", "service_account": account(), "key": key_with_secret()}).to_string()
}

/// The key-create body inside a `response` wrapper (both unwraps are covered).
fn key_create_body() -> String {
    json!({"result": "yes", "response": {"key": key_with_secret()}}).to_string()
}

const CREATE_ARGS: [&str; 8] = [
    "service-account",
    "create",
    "--workspace",
    WORKSPACE,
    "--name",
    "ci-bot",
    "--role",
    "editor",
];
const KEY_CREATE_ARGS: [&str; 6] = ["sa", "key", "create", SA_ID, "--workspace", WORKSPACE];

fn assert_secret_once(stdout: &str, format: &str) {
    assert_eq!(
        stdout.matches(SECRET).count(),
        1,
        "--format {format} must print the full secret exactly once; got:\n{stdout}"
    );
}

#[tokio::test]
async fn create_prints_the_secret_once_in_every_format() {
    for format in ALL_FORMATS {
        let (addr, seen) = spawn_stub(200, create_body()).await;
        let output = run(&addr, &["--format", format], &CREATE_ARGS).await;
        let stdout = stdout_of(&output, &format!("create --format {format}"));
        assert_secret_once(&stdout, format);
        // The account and the key both survive the rendering.
        assert!(stdout.contains(SA_ID), "--format {format}:\n{stdout}");
        assert!(stdout.contains(KEY_ID), "--format {format}:\n{stdout}");
        assert!(stdout.contains("ci-bot"), "--format {format}:\n{stdout}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("shown only once"),
            "the one-time notice goes to stderr: {stderr}"
        );
        assert!(!stderr.contains(SECRET), "the secret never reaches stderr");

        // Exactly one request, on the documented route, with only the set fields.
        let requests = seen.requests();
        assert_eq!(requests.len(), 1, "{requests:?}");
        let (method_path, body) = &requests[0];
        assert_eq!(
            method_path,
            &format!("POST /workspace/{WORKSPACE}/service_accounts/create/")
        );
        let mut fields: Vec<&str> = body.split('&').collect();
        fields.sort_unstable();
        assert_eq!(fields, vec!["name=ci-bot", "role=editor"], "{body}");
    }
}

#[tokio::test]
async fn create_json_is_one_flat_object_with_the_secret_last() {
    let (addr, _seen) = spawn_stub(200, create_body()).await;
    let output = run(
        &addr,
        &["--format", "json"],
        &[
            &CREATE_ARGS[..],
            &["--key-expires", "1y", "--key-memo", "deploys"],
        ]
        .concat(),
    )
    .await;
    let stdout = stdout_of(&output, "create --format json");
    let parsed: Value = serde_json::from_str(&stdout).expect("stdout is json");
    let obj = parsed.as_object().expect("one object");
    assert_eq!(obj.keys().next_back().map(String::as_str), Some("secret"));
    assert_eq!(obj["secret"], SECRET);
    assert_eq!(obj["service_account_id"], SA_ID);
    assert_eq!(obj["key_id"], KEY_ID);
    assert_eq!(obj["key_hint"], "fio_sa_…U1v");
    assert_eq!(obj["name"], "ci-bot");
    assert!(
        !obj.contains_key("result"),
        "envelope bookkeeping is dropped"
    );
    assert!(
        obj.values().all(|v| !v.is_object()),
        "no nested object is left for a flattener to drop: {obj:?}"
    );
}

#[tokio::test]
async fn create_sends_the_key_options_when_given() {
    let (addr, seen) = spawn_stub(200, create_body()).await;
    let output = run(
        &addr,
        &["--format", "json"],
        &[
            &CREATE_ARGS[..],
            &["--key-expires", "never", "--key-memo", "deploys"],
        ]
        .concat(),
    )
    .await;
    stdout_of(&output, "create with key options");
    let requests = seen.requests();
    let mut fields: Vec<&str> = requests[0].1.split('&').collect();
    fields.sort_unstable();
    assert_eq!(
        fields,
        vec![
            "key_expires=never",
            "key_memo=deploys",
            "name=ci-bot",
            "role=editor"
        ]
    );
}

#[tokio::test]
async fn key_create_prints_the_secret_once_in_every_format() {
    for format in ALL_FORMATS {
        let (addr, seen) = spawn_stub(200, key_create_body()).await;
        let output = run(&addr, &["--format", format], &KEY_CREATE_ARGS).await;
        let stdout = stdout_of(&output, &format!("key create --format {format}"));
        assert_secret_once(&stdout, format);
        assert!(stdout.contains(KEY_ID), "--format {format}:\n{stdout}");
        assert!(stdout.contains(SA_ID), "--format {format}:\n{stdout}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("shown only once"), "{stderr}");

        let requests = seen.requests();
        assert_eq!(requests.len(), 1, "{requests:?}");
        assert_eq!(
            requests[0].0,
            format!("POST /workspace/{WORKSPACE}/service_accounts/{SA_ID}/keys/create/")
        );
        assert_eq!(
            requests[0].1, "",
            "an omitted --expires / --memo is not sent"
        );
    }
}

#[tokio::test]
async fn key_create_json_puts_the_secret_last() {
    let (addr, seen) = spawn_stub(200, key_create_body()).await;
    let output = run(
        &addr,
        &["--format", "json"],
        &[&KEY_CREATE_ARGS[..], &["--expires", "30d", "--memo", "m"]].concat(),
    )
    .await;
    let stdout = stdout_of(&output, "key create --format json");
    let parsed: Value = serde_json::from_str(&stdout).expect("json");
    let obj = parsed.as_object().expect("object");
    assert_eq!(obj.keys().next_back().map(String::as_str), Some("secret"));
    assert_eq!(obj["service_account_id"], SA_ID);
    assert_eq!(obj["key_id"], KEY_ID);
    let mut fields: Vec<String> = seen.requests()[0].1.split('&').map(str::to_owned).collect();
    fields.sort_unstable();
    assert_eq!(fields, vec!["expires=30d", "memo=m"]);
}

/// `--quiet` and `--fields` would discard the only copy of the secret. Both
/// are refused, non-zero, with ZERO requests reaching the server.
#[tokio::test]
async fn quiet_and_fields_are_refused_before_any_request() {
    let cases: [(&[&str], &str); 4] = [
        (&["--quiet"], "--quiet"),
        (&["--fields", "name"], "--fields"),
        (&["--fields", "secret"], "--fields"),
        (&["-q", "--format", "json"], "--quiet"),
    ];
    for args in [&CREATE_ARGS[..], &KEY_CREATE_ARGS[..]] {
        for (global, flag) in cases {
            let (addr, seen) = spawn_stub(200, create_body()).await;
            let output = run(&addr, global, args).await;
            assert!(
                !output.status.success(),
                "{global:?} {args:?} must exit non-zero"
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains(flag), "{global:?}: {stderr}");
            assert!(stderr.contains("once"), "{global:?}: {stderr}");
            assert_eq!(
                seen.count(),
                0,
                "{global:?} {args:?} must be refused before any request"
            );
            assert!(output.stdout.is_empty());
        }
    }
}

/// A gateway error after the server may already have minted must NOT be
/// replayed (a replay would mint a second account or key while the first
/// secret is lost). The error explains how to recover.
#[tokio::test]
async fn mints_are_not_replayed_after_a_gateway_error() {
    for args in [&CREATE_ARGS[..], &KEY_CREATE_ARGS[..]] {
        let (addr, seen) = spawn_stub(502, "<html>Bad Gateway</html>".to_owned()).await;
        let output = run(&addr, &["--format", "json"], args).await;
        assert!(!output.status.success(), "{args:?}");
        assert_eq!(seen.count(), 1, "{args:?} must be sent exactly once");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("may have been created"), "{stderr}");
        assert!(stderr.contains("key revoke"), "{stderr}");
    }
}

/// A 307 / 308 would make a redirect-following client re-send the POST body
/// and mint again. Mints never follow: exactly ONE request, non-zero exit, and
/// the unknown-outcome recovery guidance.
#[tokio::test]
async fn mints_never_follow_a_redirect() {
    let create_path = format!("/workspace/{WORKSPACE}/service_accounts/create/");
    let key_create_path = format!("/workspace/{WORKSPACE}/service_accounts/{SA_ID}/keys/create/");
    for status in [307_u16, 308] {
        for (args, location) in [
            (&CREATE_ARGS[..], &create_path),
            (&KEY_CREATE_ARGS[..], &key_create_path),
        ] {
            let (addr, seen) =
                spawn_stub_with_location(status, String::new(), Some(location.clone())).await;
            let output = run(&addr, &["--format", "json"], args).await;
            assert!(!output.status.success(), "{status} {args:?}");
            assert_eq!(
                seen.count(),
                1,
                "{status} {args:?}: the redirect must not be followed"
            );
            assert!(output.stdout.is_empty());
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("may have been created"), "{stderr}");
            assert!(stderr.contains("redirect"), "{stderr}");
        }
    }
}

#[tokio::test]
async fn list_keeps_the_limit_for_empty_and_non_empty_lists() {
    let non_empty = json!({
        "result": "yes",
        "service_accounts": [account()],
        "limit": {"max": 5, "used": 1}
    })
    .to_string();
    let empty = json!({
        "result": "yes",
        "service_accounts": [],
        "limit": {"max": 20, "used": 0}
    })
    .to_string();
    for format in ALL_FORMATS {
        let (addr, seen) = spawn_stub(200, non_empty.clone()).await;
        let output = run(
            &addr,
            &["--format", format],
            &["sa", "list", "--workspace", WORKSPACE],
        )
        .await;
        let stdout = stdout_of(&output, &format!("list --format {format}"));
        assert!(stdout.contains("limit_max"), "--format {format}:\n{stdout}");
        assert!(stdout.contains("ci-bot"), "--format {format}:\n{stdout}");
        assert_eq!(
            seen.requests()[0].0.split('?').next(),
            Some(format!("GET /workspace/{WORKSPACE}/service_accounts/").as_str())
        );

        let (addr, _) = spawn_stub(200, empty.clone()).await;
        let output = run(
            &addr,
            &["--format", format],
            &["sa", "list", "--workspace", WORKSPACE],
        )
        .await;
        let stdout = stdout_of(&output, &format!("empty list --format {format}"));
        assert!(stdout.contains("limit_max"), "--format {format}:\n{stdout}");
        assert!(stdout.contains("20"), "--format {format}:\n{stdout}");
    }
}

#[tokio::test]
async fn info_shows_the_account_with_zero_and_many_keys() {
    let no_keys = json!({"result": "yes", "service_account": account(), "keys": []}).to_string();
    let two_keys = json!({
        "result": "yes",
        "service_account": account(),
        "keys": [key("kFirstKeyIdAAAAAAAAA"), key("kSecondKeyIdBBBBBBBB")]
    })
    .to_string();
    for format in ALL_FORMATS {
        let (addr, seen) = spawn_stub(200, no_keys.clone()).await;
        let output = run(
            &addr,
            &["--format", format],
            &["sa", "info", SA_ID, "--workspace", WORKSPACE],
        )
        .await;
        let stdout = stdout_of(&output, &format!("info (0 keys) --format {format}"));
        assert!(stdout.contains("ci-bot"), "--format {format}:\n{stdout}");
        assert!(stdout.contains(SA_ID), "--format {format}:\n{stdout}");
        assert_eq!(
            seen.requests()[0].0.split('?').next(),
            Some(format!("GET /workspace/{WORKSPACE}/service_accounts/{SA_ID}/details/").as_str())
        );

        let (addr, _) = spawn_stub(200, two_keys.clone()).await;
        let output = run(
            &addr,
            &["--format", format],
            &["sa", "info", SA_ID, "--workspace", WORKSPACE],
        )
        .await;
        let stdout = stdout_of(&output, &format!("info (2 keys) --format {format}"));
        for needle in [
            "kFirstKeyIdAAAAAAAAA",
            "kSecondKeyIdBBBBBBBB",
            "ci-bot",
            SA_ID,
        ] {
            assert!(
                stdout.contains(needle),
                "--format {format} must show {needle}:\n{stdout}"
            );
        }
    }
}

/// `delete` without `--yes` in a non-interactive run is refused before any
/// request; with `--yes` it sends one bodiless DELETE.
#[tokio::test]
async fn delete_confirms_before_the_request() {
    let (addr, seen) = spawn_stub(200, json!({"result": "yes"}).to_string()).await;
    let output = run(
        &addr,
        &["--format", "json"],
        &["sa", "delete", SA_ID, "--workspace", WORKSPACE],
    )
    .await;
    assert!(!output.status.success());
    assert_eq!(seen.count(), 0);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--yes"), "{stderr}");
    assert!(stderr.contains("disable"), "{stderr}");

    let output = run(
        &addr,
        &["--format", "json"],
        &["sa", "delete", SA_ID, "--workspace", WORKSPACE, "--yes"],
    )
    .await;
    let stdout = stdout_of(&output, "delete --yes");
    let parsed: Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(
        parsed,
        json!({"status": "deleted", "service_account_id": SA_ID})
    );
    let requests = seen.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0],
        (
            format!("DELETE /workspace/{WORKSPACE}/service_accounts/{SA_ID}/"),
            String::new()
        )
    );
}

/// `.` / `..` ids would be resolved as dot-segments — `key revoke SA ..` would
/// become the account DELETE. They are refused in every id position, before
/// any request, even with `--yes`.
#[tokio::test]
async fn dot_segment_ids_are_refused_before_any_request() {
    let cases: [&[&str]; 6] = [
        &[
            "sa",
            "key",
            "revoke",
            SA_ID,
            "..",
            "--workspace",
            WORKSPACE,
            "--yes",
        ],
        &[
            "sa",
            "key",
            "revoke",
            "..",
            "k1",
            "--workspace",
            WORKSPACE,
            "--yes",
        ],
        &["sa", "delete", ".", "--workspace", WORKSPACE, "--yes"],
        &["sa", "info", SA_ID, "--workspace", ".."],
        &["sa", "key", "create", "..", "--workspace", WORKSPACE],
        &[
            "sa",
            "create",
            "--workspace",
            "..",
            "--name",
            "n",
            "--role",
            "viewer",
        ],
    ];
    for args in cases {
        let (addr, seen) = spawn_stub(200, json!({"result": "yes"}).to_string()).await;
        let output = run(&addr, &["--format", "json"], args).await;
        assert!(!output.status.success(), "{args:?}");
        assert_eq!(seen.count(), 0, "{args:?} must not reach the server");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("must not be"), "{args:?}: {stderr}");
        assert!(
            !stderr.contains("may have been created"),
            "a local refusal is not an unknown mint outcome: {stderr}"
        );
    }
}

/// A create response without the account object still prints the key's secret
/// (exactly once), with a stderr note about the missing account details —
/// it is not reported as a lost response.
#[tokio::test]
async fn create_without_an_account_object_still_prints_the_secret() {
    let body = json!({"result": "yes", "key": key_with_secret()}).to_string();
    for format in ALL_FORMATS {
        let (addr, _seen) = spawn_stub(200, body.clone()).await;
        let output = run(&addr, &["--format", format], &CREATE_ARGS).await;
        let stdout = stdout_of(&output, &format!("create (no account) --format {format}"));
        assert_secret_once(&stdout, format);
        assert!(stdout.contains(KEY_ID), "--format {format}:\n{stdout}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("did not include the new service account"),
            "{stderr}"
        );
        assert!(!stderr.contains("may have been created"), "{stderr}");
    }
}
