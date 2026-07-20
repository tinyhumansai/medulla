//! Unit tests for child spawning: which stdio strategy is chosen, and how the
//! PTY handles are wired through to the run loop.
//!
//! The PTY path is driven with a stub [`PtySpawner`] rather than a real
//! pseudo-terminal — allocating one is the app crate's job, and its own suite
//! (`medulla-tui`'s `feature_harness_pty`) covers it against a live `/bin/sh`.

use std::collections::HashMap;

use tokio::sync::{mpsc, oneshot};

use super::child::{exit_code, spawn_child_with};
use crate::tinyplace::HarnessProvider;
use crate::wrapper::{PtyHarness, PtyRequest, WrapperConfig};

/// The receiving ends of a stub [`PtyHarness`], kept alive by the test.
struct StubPty {
    input: mpsc::UnboundedReceiver<Vec<u8>>,
    done: oneshot::Sender<i32>,
    kill: oneshot::Receiver<()>,
    requested: std::sync::Arc<std::sync::Mutex<Option<PtyRequest>>>,
}

/// A binary that exits 0 immediately, spelled for the host platform.
///
/// These tests assert on how the child is *wired up*, not on what it does, so
/// any trivially spawnable program works — it just has to exist on Windows too,
/// where the lib suite also runs.
fn noop_bin() -> (&'static str, Vec<String>) {
    if cfg!(windows) {
        ("cmd", vec!["/C".to_string(), "exit".to_string()])
    } else {
        ("/bin/echo", Vec::new())
    }
}

/// A config for `bin`, optionally carrying a spawner.
fn config(spawner: Option<crate::wrapper::PtySpawner>) -> WrapperConfig {
    WrapperConfig {
        provider: HarnessProvider::Codex,
        child_args: Vec::new(),
        env: HashMap::new(),
        cwd: "/".to_string(),
        no_bridge: true,
        session_id: None,
        pty_spawner: spawner,
    }
}

/// Build a spawner that hands back channels the test controls, plus the handles
/// needed to observe what the wrapper does with them.
fn stub_spawner() -> (crate::wrapper::PtySpawner, StubPty) {
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let (done_tx, done_rx) = oneshot::channel();
    let (kill_tx, kill_rx) = oneshot::channel();
    let (drained_tx, drained_rx) = oneshot::channel();
    let requested = std::sync::Arc::new(std::sync::Mutex::new(None));

    let seen = requested.clone();
    let spawner: crate::wrapper::PtySpawner = Box::new(move |request: PtyRequest| {
        *seen.lock().unwrap() = Some(request);
        let _ = drained_tx.send(());
        Ok(PtyHarness {
            input: input_tx,
            done: done_rx,
            kill: kill_tx,
            drained: drained_rx,
            restore: Box::new(|| {}),
        })
    });

    (
        spawner,
        StubPty {
            input: input_rx,
            done: done_tx,
            kill: kill_rx,
            requested,
        },
    )
}

/// On a terminal with injection active, the child goes to the PTY spawner and
/// its handles are surfaced on the session.
#[tokio::test]
async fn interactive_injection_uses_the_pty_spawner() {
    let (spawner, mut stub) = stub_spawner();
    let mut cfg = config(Some(spawner));
    let args = vec!["--flag".to_string()];

    let mut session = spawn_child_with("codex", &args, &mut cfg, true, true).unwrap();

    // The request carries the resolved argv and working directory verbatim.
    let request = stub.requested.lock().unwrap().take().unwrap();
    assert_eq!(request.bin, "codex");
    assert_eq!(request.args, args);
    assert_eq!(request.cwd, "/");

    // Injection reaches the PTY writer.
    session
        .input
        .as_ref()
        .unwrap()
        .send(b"hi\n".to_vec())
        .unwrap();
    assert_eq!(stub.input.recv().await.unwrap(), b"hi\n".to_vec());

    // Kill and drain/restore are plumbed through rather than dropped.
    session.kill.take().unwrap().send(()).unwrap();
    assert!(stub.kill.await.is_ok());
    assert!(session.drained.is_some());
    assert!(session.restore.is_some());

    stub.done.send(7).unwrap();
    assert_eq!(session.done.await.unwrap(), 7);
}

/// A spawner that fails must not fail the session: the child falls back to
/// inherited stdio, without injection.
#[tokio::test]
async fn pty_failure_falls_back_to_inherited_stdio() {
    let spawner: crate::wrapper::PtySpawner =
        Box::new(|_| Err(anyhow::anyhow!("no pty available")));
    let mut cfg = config(Some(spawner));

    let (bin, args) = noop_bin();
    let session = spawn_child_with(bin, &args, &mut cfg, true, true).unwrap();

    // Fallback means no writable handle on the child's input.
    assert!(session.input.is_none());
    assert!(session.drained.is_none());
    assert!(session.restore.is_none());
    assert_eq!(session.done.await.unwrap(), 0);
}

/// Off a terminal, injection still works — over a plain pipe, which is all a
/// non-interactive harness needs. The spawner is left untouched.
#[tokio::test]
async fn non_interactive_injection_uses_a_pipe() {
    let (spawner, stub) = stub_spawner();
    let mut cfg = config(Some(spawner));

    let (bin, args) = noop_bin();
    let session = spawn_child_with(bin, &args, &mut cfg, true, false).unwrap();

    assert!(
        stub.requested.lock().unwrap().is_none(),
        "pty not allocated"
    );
    assert!(session.input.is_some(), "pipe still accepts injection");
    assert!(session.restore.is_none());
}

/// With injection off the child simply inherits our stdio.
#[tokio::test]
async fn no_injection_inherits_stdio() {
    let mut cfg = config(None);
    let (bin, args) = noop_bin();
    let session = spawn_child_with(bin, &args, &mut cfg, false, true).unwrap();
    assert!(session.input.is_none());
    assert_eq!(session.done.await.unwrap(), 0);
}

/// A missing binary is reported as an error by the stdio path.
#[tokio::test]
async fn missing_binary_errors() {
    let mut cfg = config(None);
    let result = spawn_child_with("/definitely/not/a/binary", &[], &mut cfg, false, false);
    assert!(result.is_err());
}

/// Signal deaths map to the shell's `128 + signal`.
#[cfg(unix)]
#[test]
fn signal_exit_maps_to_shell_convention() {
    use std::os::unix::process::ExitStatusExt;
    // Raw wait status 9 == killed by SIGKILL, with no exit code of its own.
    assert_eq!(exit_code(std::process::ExitStatus::from_raw(9)), 128 + 9);
    assert_eq!(exit_code(std::process::ExitStatus::from_raw(0)), 0);
}
