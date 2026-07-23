//! Manager tests: the session lifecycle, the bounded/unbound turn split, and
//! the transcript the Sessions tab renders.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use crate::daemon::providers::{RunTaskFn, RunTaskResult};
use crate::tinyplace::HarnessProvider;

use super::super::input::{Folded, Observation};
use super::super::manager::{OpenSession, SessionConfig, SessionManager, TranscriptRole};
use super::super::routing::Transport;
use super::super::types::{
    SessionClass, SessionDriver, SessionKey, SessionPhase, TurnOrigin, TurnRequest,
};
use super::input_tests::prompt_envelope;

// ---------------------------------------------------------------- manager ---

/// A clock that advances a fixed step on every read, so ordering is observable.
fn stub_clock() -> crate::sessions::manager::NowFn {
    let counter = Arc::new(AtomicI64::new(1_000));
    Arc::new(move || counter.fetch_add(1, Ordering::SeqCst))
}

/// An executor that records the prompts and resume ids it was handed.
#[allow(clippy::type_complexity)]
fn recording_executor(
    reply: &str,
    session_id: Option<&str>,
) -> (RunTaskFn, Arc<Mutex<Vec<(String, Option<String>)>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let reply = reply.to_string();
    let session_id = session_id.map(str::to_string);
    let run: RunTaskFn = {
        let seen = seen.clone();
        Arc::new(move |options| {
            seen.lock()
                .unwrap()
                .push((options.prompt.clone(), options.resume_session_id.clone()));
            let reply = reply.clone();
            let session_id = session_id.clone();
            let provider = options.provider;
            Box::pin(async move {
                Ok(RunTaskResult {
                    provider,
                    reply,
                    events: 1,
                    usage: None,
                    session_id,
                })
            })
        })
    };
    (run, seen)
}

fn manager(run: RunTaskFn) -> SessionManager {
    SessionManager::new(
        SessionConfig {
            // codex routes unbound sessions onto the one-shot transport, which
            // is what lets these tests exercise continuity without a real CLI.
            default_provider: HarnessProvider::Codex,
            ..SessionConfig::default()
        },
        run,
    )
    .with_now(stub_clock())
}

/// A manager whose default provider is `claude` — the one provider that routes
/// unbound sessions onto the interactive transport. Its empty env means the
/// child inherits no `PATH`, so spawning the harness fails deterministically
/// whether or not a real `claude` is installed.
fn claude_manager(run: RunTaskFn) -> SessionManager {
    SessionManager::new(SessionConfig::default(), run).with_now(stub_clock())
}

/// An executor that blocks each turn until `release` is notified, so a test can
/// observe a session while it is genuinely mid-turn.
fn gated_executor() -> (RunTaskFn, Arc<tokio::sync::Notify>) {
    let release = Arc::new(tokio::sync::Notify::new());
    let run: RunTaskFn = {
        let release = release.clone();
        Arc::new(move |options| {
            let release = release.clone();
            let provider = options.provider;
            Box::pin(async move {
                release.notified().await;
                Ok(RunTaskResult {
                    provider,
                    reply: "done".to_string(),
                    events: 1,
                    usage: None,
                    session_id: None,
                })
            })
        })
    };
    (run, release)
}

/// Build a manager whose `claude` provider resolves to a fake `/bin/sh` harness
/// running `body`, so the live interactive transport can be driven offline —
/// exactly as `interactive::tests` drives the process directly, but here through
/// the whole [`SessionManager`] path (spawn, stream, settle, teardown).
///
/// The script is pointed at via `TINYVERSE_CLAUDE_BIN`, which
/// [`provider_bin`](crate::tinyplace::env::provider_bin) honors first; `PATH` is
/// forwarded so the script's `printf`/`sleep`/etc. resolve after the child's env
/// is cleared. Returns the tempdir too — dropping it deletes the script, so the
/// caller must keep it alive for the test's duration.
///
/// `pub(super)` so the sibling `ops_tests` can drive `apply` over the same live
/// transport (the `use super::input_tests::prompt_envelope` pattern in reverse).
#[cfg(unix)]
pub(super) fn claude_harness_manager(body: &str) -> (SessionManager, tempfile::TempDir) {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fake-claude");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut env = std::collections::HashMap::new();
    env.insert(
        "PATH".to_string(),
        std::env::var("PATH").unwrap_or_default(),
    );
    env.insert(
        "TINYVERSE_CLAUDE_BIN".to_string(),
        path.to_string_lossy().into_owned(),
    );
    let manager = SessionManager::new(
        SessionConfig {
            default_provider: HarnessProvider::Claude,
            workspace: dir.path().to_string_lossy().into_owned(),
            env,
            ..SessionConfig::default()
        },
        // The interactive transport never calls the one-shot executor; make that
        // an assertion rather than a silent fallback.
        Arc::new(|_| Box::pin(async { Err("one-shot executor must not run".to_string()) })),
    )
    .with_now(stub_clock());
    (manager, dir)
}

/// Poll a session's phase until it reaches `target`, failing rather than
/// hanging if it never does.
async fn wait_for_phase(manager: &SessionManager, id: &str, target: SessionPhase) {
    for _ in 0..2_000 {
        if manager.record(id).map(|record| record.phase) == Some(target) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    panic!("session {id} never reached {target:?}");
}

/// Spawn `submit(id, text)` on a background task so a test can inspect the
/// session while the turn is genuinely in flight.
#[allow(clippy::type_complexity)]
fn submit_in_background(
    manager: &SessionManager,
    id: &str,
    text: &str,
) -> tokio::task::JoinHandle<Result<crate::sessions::types::TurnOutcome, String>> {
    let manager = manager.clone();
    let (id, text) = (id.to_string(), text.to_string());
    tokio::spawn(async move { manager.submit(&id, &text).await })
}

mod lifecycle;
mod turns;
