//! End-to-end lock for the e-signature gate: every `fastio sign` command is
//! refused before configuration or network access.
//!
//! Drives the real binary against a loopback listener and asserts the command
//! exits 1 with the disabled message and never opens a connection. Isolation
//! (throwaway `HOME`, removed proxy variables, loopback `NO_PROXY`) follows
//! `tests/service_account_wire.rs`.

use std::io::ErrorKind;
use std::net::TcpListener;
use std::process::Command;

/// A throwaway `HOME` so the child never reads the developer's real profile.
fn isolated_home() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("fastio-sign-gate-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create isolated home");
    dir
}

#[test]
fn sign_command_is_refused_without_network() {
    assert_refused_without_network(&[
        "sign",
        "envelope",
        "list",
        "--workspace",
        "1234567890123456789",
    ]);
}

/// The outward-facing `send` (which would email recipients) is refused the
/// same way, even with `--yes` skipping the confirmation prompt.
#[test]
fn sign_send_is_refused_without_network() {
    assert_refused_without_network(&[
        "sign",
        "envelope",
        "send",
        "--workspace",
        "1234567890123456789",
        "envelope-id",
        "--yes",
    ]);
}

/// Run `fastio <args>` against a loopback listener and assert the e-signature
/// refusal: exit 1, the disabled message on stderr, and no connection opened.
fn assert_refused_without_network(args: &[&str]) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
    let addr = listener.local_addr().expect("listener addr");

    let output = Command::new(env!("CARGO_BIN_EXE_fastio"))
        .arg("--api-base")
        .arg(format!("http://{addr}"))
        .arg("--token")
        .arg("test-token")
        .arg("--no-color")
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
        .expect("run the fastio binary");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "exit code; stderr: {stderr}");
    assert!(
        stderr.contains("E-signatures are no longer available."),
        "stderr must carry the disabled message: {stderr}"
    );

    listener
        .set_nonblocking(true)
        .expect("set listener nonblocking");
    match listener.accept() {
        Err(e) if e.kind() == ErrorKind::WouldBlock => {}
        Ok((_, peer)) => panic!("the sign gate must not connect, but {peer} did"),
        Err(e) => panic!("unexpected accept error: {e}"),
    }
}
