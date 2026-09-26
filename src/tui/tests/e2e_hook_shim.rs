//! The whole reporting chain, with nothing faked in the middle: a real
//! `medulla hook` process, the real control socket, and the real log the Hooks
//! page renders.
//!
//! This is the test that would have caught the feature being inert. Every part
//! of it can be unit-tested and still leave the operator with a harness that
//! reports nothing, because the failures live in the seams — an argv the harness
//! never runs, an env var the shim cannot read, a socket op the server does not
//! route.

#![cfg(unix)]

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use medulla::control_socket::server::{ControlServer, FleetDefaults, HubFleetOps};
use medulla::control_socket::{Grant, GrantRegistry};
use medulla::harness_hooks::{HookEvent, HookEventLog};

/// Bind a control server on a scratch socket, with `log` as its hook sink.
async fn serve(dir: &std::path::Path, log: HookEventLog) -> (ControlServer, String) {
    let grants = GrantRegistry::new();
    // The credential a launched harness's built-in hooks actually get: see
    // `medulla::mcp::attach::local_hook_grant`.
    let token = grants.mint(Grant::hook_only("pty-under-test"));
    let ops = Arc::new(
        HubFleetOps::new(
            Default::default(),
            FleetDefaults {
                worker_address: None,
            },
        )
        .with_hook_log(log),
    );
    let server = ControlServer::bind(&dir.join("control.sock"), ops, grants)
        .await
        .expect("the control socket binds");
    (server, token)
}

/// Run `medulla hook <event>` the way a harness would: payload on stdin, the
/// spawn's own grant in the environment.
fn run_shim(
    event: &str,
    socket: &std::path::Path,
    token: &str,
    payload: &str,
) -> std::process::ExitStatus {
    let mut child = Command::new(env!("CARGO_BIN_EXE_medulla"))
        .args(["hook", event])
        .env("MEDULLA_HOOK_SOCKET", socket)
        .env("MEDULLA_HOOK_GRANT", token)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the shim starts");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.as_bytes())
        .expect("write the payload");
    let output = child.wait_with_output().expect("the shim exits");
    assert!(
        output.stdout.is_empty(),
        "a hook's stdout is a decision protocol; the shim must write nothing: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    output.status
}

/// Wait for the server to record `count` reports.
fn settle(log: &HookEventLog, count: u64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while log.recorded() < count && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lifecycle_report_travels_from_the_shim_to_the_log() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log = HookEventLog::new();
    let (server, token) = serve(dir.path(), log.clone()).await;

    let status = run_shim(
        "PostToolUse",
        server.path(),
        &token,
        r#"{"session_id":"abc123","cwd":"/repo","tool_name":"Edit","tool_input":{"file_path":"/repo/secret.env"}}"#,
    );
    assert!(
        status.success(),
        "a hook must never fail its harness's turn"
    );

    settle(&log, 1);
    let recorded = log.recent(10);
    assert_eq!(recorded.len(), 1, "the report never reached the log");
    let report = &recorded[0];
    // Filed under the grant's session, which is how the Hooks page and the rail
    // tie a report back to the pane that produced it.
    assert_eq!(report.session, "pty-under-test");
    assert_eq!(report.event, HookEvent::PostToolUse);
    assert_eq!(report.summary, "used Edit");
    assert_eq!(report.cwd.as_deref(), Some("/repo"));
    assert_eq!(report.harness_session_id.as_deref(), Some("abc123"));
    assert!(
        !format!("{report:?}").contains("secret.env"),
        "the tool's input must not cross the socket: {report:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_harness_with_no_medulla_to_report_to_still_succeeds() {
    // The ordinary case for a session started by hand, or on a host with fleet
    // tools off: the same hook command runs, finds no grant, and exits without
    // costing the turn anything.
    let mut child = Command::new(env!("CARGO_BIN_EXE_medulla"))
        .args(["hook", "Stop"])
        .env_remove("MEDULLA_HOOK_SOCKET")
        .env_remove("MEDULLA_HOOK_GRANT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the shim starts");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"{}")
        .expect("write the payload");
    let output = child.wait_with_output().expect("the shim exits");
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_blocking_env_file_does_not_delay_the_shim() {
    // The workspace `.env` can be a FIFO (or a device): an unbounded read there
    // would hang the process before `run_hook_cmd`'s own deadline starts, so
    // the harness would kill the shim as a hung hook. Regression for the
    // dotenv-before-dispatch ordering fixed on PR #192.
    let dir = tempfile::tempdir().expect("tempdir");
    let fifo = dir.path().join(".env");
    let fifo_status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo is available on unix");
    assert!(fifo_status.success(), "mkfifo failed");

    let mut child = Command::new(env!("CARGO_BIN_EXE_medulla"))
        .args(["hook", "Stop"])
        .current_dir(dir.path())
        .env_remove("MEDULLA_HOOK_SOCKET")
        .env_remove("MEDULLA_HOOK_GRANT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the shim starts");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"{}")
        .expect("write the payload");

    // A regression blocks forever on the FIFO; kill after the budget so the
    // suite fails loudly instead of hanging. A healthy shim exits in moments.
    let deadline = Instant::now() + Duration::from_secs(3);
    let status = loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("the shim blocked on the .env FIFO instead of exiting promptly");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(
        status.success(),
        "a hook must never fail its harness's turn"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_event_name_is_dropped_rather_than_recorded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log = HookEventLog::new();
    let (server, token) = serve(dir.path(), log.clone()).await;

    let status = run_shim("NotAnEvent", server.path(), &token, "{}");
    assert!(status.success());
    // Nothing to wait for; give a report that should not exist time to arrive.
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(log.recorded(), 0);
}
