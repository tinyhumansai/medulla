//! Unit tests for child spawning: which stdio strategy is chosen, and how the
//! PTY handles are wired through to the run loop.
//!
//! The PTY path is driven with a stub [`PtySpawner`] rather than a real
//! pseudo-terminal — allocating one is the app crate's job, and its own suite
//! (`medulla-tui`'s `feature_harness_pty`) covers it against a live `/bin/sh`.

use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use super::child::{exit_code, spawn_child_with};
use crate::protocol::HarnessProvider;
use crate::wrapper::{PtyHarness, PtyRequest, WrapperConfig};

mod types;
use types::StubPty;

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
        attribution: true,
        hooks: crate::harness_hooks::HooksConfig::default(),
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
    assert_eq!(
        request.env_remove,
        vec!["OPENHUMAN_WORKSPACE"],
        "the PTY child, not the Medulla process, drops the core workspace"
    );

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

// ---------------------------------------------------------------------------
// Termination-signal source
// ---------------------------------------------------------------------------

/// Send `signal` to this process.
#[cfg(unix)]
fn raise(signal: libc::c_int) {
    assert_eq!(unsafe { libc::raise(signal) }, 0, "raise({signal}) failed");
}

/// The run loop re-enters its `select!` after every branch, so the signal arm
/// is polled again once a signal has already fired.
///
/// The regression this pins: the arm was a once-built `async` block, and
/// polling a completed one panics with "`async fn` resumed after completion".
/// That panic unwound past the terminal restore and the `session_end`
/// lifecycle event, and left the harness child running with nobody waiting on
/// it. [`Signals::recv`] must therefore stay drivable for the whole session.
#[cfg(unix)]
#[tokio::test]
async fn the_signal_arm_survives_being_polled_after_a_signal() {
    let mut signals = super::Signals::install();
    let mut ticks = tokio::time::interval(Duration::from_millis(5));
    let mut fired = 0;

    raise(libc::SIGINT);
    let looped = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                _ = signals.recv() => {
                    fired += 1;
                    if fired == 2 {
                        break;
                    }
                    // Back around the loop: the arm is polled a second time.
                    raise(libc::SIGTERM);
                }
                _ = ticks.tick() => {}
            }
        }
    })
    .await;

    assert!(looped.is_ok(), "the loop stalled waiting for a signal");
    assert_eq!(fired, 2, "both signals were observed");
}

/// A host where no handler could be installed must simply never fire, rather
/// than resolving instantly and spinning the run loop.
#[tokio::test]
async fn an_unavailable_signal_source_never_fires() {
    let mut signals = super::Signals::Unavailable;
    assert!(
        tokio::time::timeout(Duration::from_millis(50), signals.recv())
            .await
            .is_err(),
        "Unavailable must never resolve"
    );
}

// ---------------------------------------------------------------------------
// Attribution env-merge wiring tests
// ---------------------------------------------------------------------------

/// Every provider's env map must be augmented with `MEDULLA_ATTRIBUTION` and
/// the `core.hooksPath` overrides — including Claude, whose own
/// `attribution.commit` setting only *asks* the model for the trailer and so
/// cannot be relied on alone.
#[cfg(unix)]
#[test]
fn attribution_env_is_merged_for_every_provider() {
    for provider in [
        HarnessProvider::Claude,
        HarnessProvider::Codex,
        HarnessProvider::Opencode,
    ] {
        let mut config = config(None);
        config.provider = provider;
        super::merge_attribution_env_into_config(&mut config);

        assert!(
            config.env.contains_key("MEDULLA_ATTRIBUTION"),
            "{provider:?} should have MEDULLA_ATTRIBUTION in env"
        );
        assert!(
            config.env.contains_key("GIT_CONFIG_VALUE_0"),
            "{provider:?} should have hooksPath in env"
        );
    }
}

/// Attribution turned off in config must leave the env map untouched for every
/// provider.
#[test]
fn attribution_env_not_merged_when_config_disables_it() {
    for provider in [
        HarnessProvider::Claude,
        HarnessProvider::Codex,
        HarnessProvider::Opencode,
    ] {
        let mut config = config(None);
        config.provider = provider;
        config.attribution = false;
        let env_before = config.env.clone();
        super::merge_attribution_env_into_config(&mut config);
        assert_eq!(config.env, env_before, "{provider:?} env must be unchanged");
    }
}
