//! (Unix-only: spawns `/bin/sh` on a pseudo-terminal.)
#![cfg(unix)]

//! Behaviour of the app-side PTY spawner ([`medulla_tui::harness_pty`]).
//!
//! The regression these guard is `medulla codex` refusing to start: when remote
//! input was enabled the wrapper handed the child a pipe on stdin, and a
//! full-screen harness exits with `stdin is not a terminal` rather than run. The
//! central assertion here is therefore [`child_sees_a_tty`] — everything else
//! covers the handles the wrapper drives the session with.

use std::collections::HashMap;

use medulla::wrapper::{PtyHarness, PtyRequest};
use medulla_tui::harness_pty::spawner;

/// Run `/bin/sh -c script` on a PTY with a minimal environment.
fn spawn_sh(script: &str) -> anyhow::Result<PtyHarness> {
    let mut env = HashMap::new();
    // Keep the child's environment predictable but usable: `sh` needs a PATH,
    // and a terminal-aware child needs a TERM.
    env.insert("PATH".to_string(), "/usr/bin:/bin".to_string());
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    spawner()(PtyRequest {
        bin: "/bin/sh".to_string(),
        args: vec!["-c".to_string(), script.to_string()],
        cwd: "/".to_string(),
        env,
    })
}

/// Await an exit code, failing the test rather than hanging forever.
async fn exit_code(harness: PtyHarness) -> i32 {
    tokio::time::timeout(std::time::Duration::from_secs(10), harness.done)
        .await
        .expect("child should exit before the timeout")
        .expect("exit code sender should not be dropped")
}

/// The bug in one assertion: a child spawned this way must see a terminal on
/// stdin, which is exactly what a piped stdin failed to provide.
#[tokio::test]
async fn child_sees_a_tty() {
    let harness = spawn_sh("[ -t 0 ] || exit 3; [ -t 1 ] || exit 4; exit 0").unwrap();
    assert_eq!(exit_code(harness).await, 0, "child should have a tty");
}

/// Bytes written to `input` reach the child's stdin.
#[tokio::test]
async fn input_reaches_the_child() {
    // `stty -echo` keeps the pty from echoing the injected line into the test's
    // own stdout; the exit code carries the result instead.
    let harness =
        spawn_sh("stty -echo; read line; [ \"$line\" = ping ] && exit 0; exit 5").unwrap();
    harness.input.send(b"ping\n".to_vec()).unwrap();
    assert_eq!(exit_code(harness).await, 0, "child should have read 'ping'");
}

/// A non-zero exit propagates verbatim.
#[tokio::test]
async fn exit_code_propagates() {
    let harness = spawn_sh("exit 42").unwrap();
    assert_eq!(exit_code(harness).await, 42);
}

/// `drained` resolves once the child's output has been copied out, which is what
/// the wrapper waits on before restoring the terminal.
#[tokio::test]
async fn drained_resolves_after_exit() {
    let harness = spawn_sh("stty -echo; exit 0").unwrap();
    let drained = harness.drained;
    let done = harness.done;
    done.await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), drained)
        .await
        .expect("reader should finish once the child exits")
        .expect("drain sender should not be dropped");
}

/// Sending on `kill` terminates a child that would otherwise outlive the test.
#[tokio::test]
async fn kill_terminates_the_child() {
    let harness = spawn_sh("sleep 30").unwrap();
    let PtyHarness { kill, done, .. } = harness;
    kill.send(()).unwrap();
    let code = tokio::time::timeout(std::time::Duration::from_secs(10), done)
        .await
        .expect("killed child should exit before the timeout")
        .expect("exit code sender should not be dropped");
    assert_ne!(code, 0, "a killed child should not report success");
}

/// A missing binary surfaces as an error, which is the wrapper's cue to fall
/// back to inherited stdio instead of failing the session.
#[tokio::test]
async fn missing_binary_is_an_error() {
    let result = spawner()(PtyRequest {
        bin: "/definitely/not/a/real/binary".to_string(),
        args: Vec::new(),
        cwd: "/".to_string(),
        env: HashMap::new(),
    });
    assert!(result.is_err(), "spawning a missing binary should fail");
}
