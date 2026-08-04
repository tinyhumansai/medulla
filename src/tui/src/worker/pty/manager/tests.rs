//! Unit tests for PTY manager bookkeeping rules.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use portable_pty::{Child, ChildKiller, ExitStatus};

use super::open::LaunchGuard;
use super::session::consumed_bell_count;

#[test]
fn a_stale_bell_sample_cannot_move_the_watermark_backwards() {
    // Release sampled one bell, then an in-flight classifier stored the second
    // before release reacquired the sessions lock.
    assert_eq!(consumed_bell_count(2, 1), 2);
}

/// A stand-in child that records whether it was asked to die, since the real
/// `spawn_command`/`try_clone_reader`/`take_writer` sequence `open` runs is
/// not something a test can reliably make fail partway through.
#[derive(Debug)]
struct RecordingChild {
    killed: Arc<AtomicBool>,
    reaped: Arc<AtomicBool>,
}

impl ChildKiller for RecordingChild {
    fn kill(&mut self) -> std::io::Result<()> {
        self.killed.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(RecordingChild {
            killed: self.killed.clone(),
            reaped: self.reaped.clone(),
        })
    }
}

impl Child for RecordingChild {
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        Ok(None)
    }

    fn wait(&mut self) -> std::io::Result<ExitStatus> {
        self.reaped.store(true, Ordering::SeqCst);
        Ok(ExitStatus::with_exit_code(0))
    }

    fn process_id(&self) -> Option<u32> {
        None
    }

    // Only required on Windows, where `portable_pty::Child` has no default —
    // this stand-in names no real process, so there is no handle to report.
    #[cfg(windows)]
    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
        None
    }
}

/// The property behind the "Revoke grants when PTY setup fails" fix: a
/// `LaunchGuard` dropped while still armed — meaning `open` returned early
/// through one of the fallible calls between `spawn_command` and the
/// `SessionHandle` that would otherwise track this child — must kill the
/// child it already spawned. Left running, that child would keep holding a
/// grant nothing has revoked and nothing else can reap.
#[test]
fn dropping_an_armed_launch_guard_kills_its_child() {
    let killed = Arc::new(AtomicBool::new(false));
    let reaped = Arc::new(AtomicBool::new(false));
    let child: Box<dyn Child + Send + Sync> = Box::new(RecordingChild {
        killed: killed.clone(),
        reaped: reaped.clone(),
    });
    let mut guard = LaunchGuard::new(Some("test-session".to_string()));
    guard.set_child(child);

    drop(guard);

    assert!(
        killed.load(Ordering::SeqCst),
        "an abandoned launch must kill the child it already spawned"
    );
    // Killing is only half of it. This guard is the child's sole owner on the
    // early-return path — the `SessionHandle::reap` that normally waits was
    // never built — so a kill without a wait leaves a zombie holding a
    // process slot until Medulla exits, which is worst exactly when pty setup
    // is failing repeatedly.
    assert!(
        reaped.load(Ordering::SeqCst),
        "a killed child must also be waited on, or it lingers as a zombie"
    );
}

/// The success path must not kill the very child it just spawned: `disarm`
/// hands it to the `SessionHandle` that becomes its permanent owner.
#[test]
fn disarming_a_launch_guard_hands_back_the_child_without_killing_it() {
    let killed = Arc::new(AtomicBool::new(false));
    let reaped = Arc::new(AtomicBool::new(false));
    let child: Box<dyn Child + Send + Sync> = Box::new(RecordingChild {
        killed: killed.clone(),
        reaped: reaped.clone(),
    });
    let mut guard = LaunchGuard::new(None);
    guard.set_child(child);

    let handed_back = guard.disarm();

    assert!(
        handed_back.is_some(),
        "the child must be handed to its permanent owner"
    );
    assert!(
        !killed.load(Ordering::SeqCst),
        "a successful launch must not kill its own child"
    );
    assert!(
        !reaped.load(Ordering::SeqCst),
        "nor wait on it — the handle taking ownership is what reaps it later"
    );
}

/// A guard armed with no child at all (a launch that failed before
/// `spawn_command` even ran) must not panic on drop.
#[test]
fn dropping_a_childless_launch_guard_is_harmless() {
    drop(LaunchGuard::new(Some("test-session-no-child".to_string())));
}
