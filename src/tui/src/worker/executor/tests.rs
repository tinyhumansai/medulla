//! Focused tests for PTY executor lifecycle decisions.

use medulla::sessions::SessionClass;

use std::collections::HashMap;

use super::hold::handback_prompt;
use super::run::{
    retains_finished_session, retains_workspace_context, retire_stopped_workspace_context,
};
use crate::worker::pty::SessionControl;

#[test]
fn mapper_context_survives_only_for_live_reusable_sessions() {
    assert!(retains_workspace_context(
        SessionClass::Unbound,
        Some(SessionControl::Orchestrator),
        true,
    ));
    assert!(retains_workspace_context(
        SessionClass::Bounded,
        Some(SessionControl::User),
        true,
    ));
    assert!(!retains_workspace_context(
        SessionClass::Bounded,
        Some(SessionControl::Orchestrator),
        true,
    ));
    assert!(!retains_workspace_context(
        SessionClass::Unbound,
        Some(SessionControl::Orchestrator),
        false,
    ));
}

#[test]
fn a_failed_orchestrator_stop_keeps_operator_owned_mapper_context() {
    let mut context = HashMap::from([(
        "pty-1".to_string(),
        (
            Some("/repo/worktrees/fix".to_string()),
            Some("fix".to_string()),
            Some("https://github.com/acme/repo/pull/42".to_string()),
        ),
    )]);
    retire_stopped_workspace_context(&mut context, "pty-1", false);
    assert!(context.contains_key("pty-1"));

    retire_stopped_workspace_context(&mut context, "pty-1", true);
    assert!(!context.contains_key("pty-1"));
}

#[test]
fn a_multi_line_instruction_still_yields_a_one_line_prompt_with_its_tail() {
    // The prompt is typed into a composer, so a line-oriented harness reads it
    // up to the first newline. An instruction carrying one — a bulleted brief, a
    // pasted trace — used to submit everything before it and silently drop the
    // rest of the sentence, which is where the do-not-redo guidance lives: the
    // one thing the hand-back turn exists to say.
    let prompt = handback_prompt("fix the parser\n- keep the tests green\n\n- then report");

    assert!(
        !prompt.contains('\n'),
        "the prompt is one line or it is truncated: {prompt:?}"
    );
    assert!(
        prompt.contains("fix the parser - keep the tests green - then report"),
        "the whole instruction survives, flattened: {prompt:?}"
    );
    assert!(
        prompt.contains("do not redo it"),
        "and so does the tail after it: {prompt:?}"
    );
    assert!(prompt.contains("complete it."), "to the end: {prompt:?}");

    // A single-line instruction is passed through unchanged.
    assert!(handback_prompt("fix the parser").contains("this task: fix the parser — if the"));
}

#[test]
fn a_finished_turn_is_kept_only_when_somebody_asked_for_it() {
    use medulla::daemon::providers::RunTaskOrigin;

    let orchestrator = Some(SessionControl::Orchestrator);

    // The case retention was built for, unchanged: a peer's delegated task
    // answered, and whoever delegated it may attach to the pane and read it.
    assert!(retains_finished_session(
        SessionClass::Bounded,
        orchestrator,
        true,
        RunTaskOrigin::DelegatedTask,
    ));

    // The narrowing. Nobody is behind either of these, so there is no reader to
    // hold a live harness process open for.
    assert!(!retains_finished_session(
        SessionClass::Bounded,
        orchestrator,
        true,
        RunTaskOrigin::Workflow,
    ));
    assert!(!retains_finished_session(
        SessionClass::Bounded,
        orchestrator,
        true,
        RunTaskOrigin::CapabilityProbe,
    ));

    // Everything else retention already refused still refuses, whatever the
    // origin: a turn that never answered, an unbound conversation, and a
    // session an operator has taken.
    assert!(!retains_finished_session(
        SessionClass::Bounded,
        orchestrator,
        false,
        RunTaskOrigin::DelegatedTask,
    ));
    assert!(!retains_finished_session(
        SessionClass::Unbound,
        orchestrator,
        true,
        RunTaskOrigin::DelegatedTask,
    ));
    assert!(!retains_finished_session(
        SessionClass::Bounded,
        Some(SessionControl::User),
        true,
        RunTaskOrigin::DelegatedTask,
    ));
}

#[test]
fn an_operator_who_adopts_an_operator_less_session_keeps_it() {
    use medulla::daemon::providers::RunTaskOrigin;

    // A workflow node's session that a person takes mid-flight is theirs from
    // that moment. It must not be retained (it is not leftover work, it is
    // somebody's session) and it must not be closed either — which is what the
    // `User` guard already buys, and what this pins so the narrowing above
    // cannot quietly start killing sessions out from under a reader.
    assert!(!retains_finished_session(
        SessionClass::Bounded,
        Some(SessionControl::User),
        true,
        RunTaskOrigin::Workflow,
    ));
}

/// The child processes a finished turn must or must not leave behind.
///
/// Driven against a real child on a real pty — `/bin/sh`, not a coding agent —
/// because the substance of the leak was an OS process nobody could see, and a
/// test that only reads the row model would have passed throughout.
#[cfg(unix)]
mod children {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    use medulla::daemon::providers::RunTaskOrigin;
    use medulla::protocol::HarnessProvider;
    use medulla::sessions::SessionClass;

    use crate::worker::executor::types::PtySessionExecutor;
    use crate::worker::pty::{CloseAttempt, LaunchSpec, PtyManager, SessionControl, SessionOrigin};

    /// A spec whose child appends a line to `marker` twice a second forever.
    ///
    /// The file is the evidence: a killed child stops writing, and a child that
    /// is merely un-drawn does not.
    fn ticker(marker: &std::path::Path) -> LaunchSpec {
        let mut env = HashMap::new();
        if let Ok(path) = std::env::var("PATH") {
            env.insert("PATH".to_string(), path);
        }
        env.insert("TERM".to_string(), "xterm-256color".to_string());
        LaunchSpec {
            // Codex takes no preset session id, so its interactive argv is empty
            // and the script below is the whole command.
            provider: HarnessProvider::Codex,
            preset: None,
            bin: "/bin/sh".to_string(),
            cwd: "/".to_string(),
            env,
            extra_args: vec![
                "-c".to_string(),
                format!(
                    "while true; do echo tick >> {}; sleep 0.05; done",
                    marker.display()
                ),
            ],
            skip_permissions: false,
            label: "test".to_string(),
            session_id: None,
            model: None,
            control: SessionControl::Orchestrator,
            origin: SessionOrigin::Orchestrator,
            name: None,
            mcp_grant_session: None,
        }
    }

    /// A spec whose child exits immediately on its own, leaving nothing to
    /// interrupt or kill — the shape of a harness that finished naturally,
    /// before anyone got around to processing its completion.
    fn short_lived() -> LaunchSpec {
        LaunchSpec {
            provider: HarnessProvider::Codex,
            preset: None,
            bin: "/bin/true".to_string(),
            cwd: "/".to_string(),
            env: HashMap::new(),
            extra_args: Vec::new(),
            skip_permissions: false,
            label: "test".to_string(),
            session_id: None,
            model: None,
            control: SessionControl::Orchestrator,
            origin: SessionOrigin::Orchestrator,
            name: None,
            mcp_grant_session: None,
        }
    }

    fn ticks(marker: &std::path::Path) -> usize {
        std::fs::read_to_string(marker)
            .map(|s| s.lines().count())
            .unwrap_or(0)
    }

    fn wait_for(what: &str, mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if check() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out after 30s waiting for: {what}");
    }

    fn executor(sessions: PtyManager) -> PtySessionExecutor {
        PtySessionExecutor::new(sessions, HashMap::new(), "/".to_string())
    }

    #[test]
    fn a_workflow_nodes_finished_session_leaves_no_harness_process_running() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let marker = dir.path().join("ticks");
        let sessions = PtyManager::new();
        let id = sessions.open(ticker(&marker)).expect("a live child");
        wait_for("the child to start writing", || ticks(&marker) > 0);

        executor(sessions.clone()).finish_turn(
            &id,
            SessionClass::Bounded,
            true,
            RunTaskOrigin::Workflow,
        );

        // The row stops being live...
        wait_for("the session to stop running", || {
            sessions.row(&id).is_some_and(|row| !row.state.is_running())
        });
        // ...the manager agrees the child is gone — `forget` refuses while one
        // is alive, so a successful forget is that refusal not firing...
        wait_for("the manager to release the session", || {
            sessions.forget(&id)
        });
        // ...and, the part that matters, the process itself has stopped doing
        // work. Sampled twice across a window many times its write interval.
        let before = ticks(&marker);
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(
            ticks(&marker),
            before,
            "the harness process must actually be dead, not merely off the rail"
        );
        assert!(
            sessions.row(&id).is_none(),
            "and its row is gone with it, rather than sitting there marked done"
        );
    }

    #[test]
    fn a_delegated_tasks_finished_session_is_still_kept_standing() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let marker = dir.path().join("ticks");
        let sessions = PtyManager::new();
        let id = sessions.open(ticker(&marker)).expect("a live child");
        wait_for("the child to start writing", || ticks(&marker) > 0);

        executor(sessions.clone()).finish_turn(
            &id,
            SessionClass::Bounded,
            true,
            RunTaskOrigin::DelegatedTask,
        );

        let row = sessions.row(&id).expect("the session is still listed");
        assert!(
            row.state.is_running(),
            "a delegated task's finished session keeps its harness for the operator"
        );
        assert!(
            row.retained,
            "and is flagged for the `read and release` cue"
        );

        let before = ticks(&marker);
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            ticks(&marker) > before,
            "its child is genuinely still alive"
        );

        sessions.close(&id);
    }

    /// Same finished, operator-less turn as above, except the operator adopts
    /// the session first. `finish_turn` snapshots `control()` well before its
    /// non-retention branch would call `close_if_orchestrator`, so with control
    /// already `User` at that snapshot the close is never attempted at all —
    /// this exercises the same `settle_or_hand_back` cleanup the close-loses
    /// path (below) shares, deterministically: `busy` clears so a later
    /// `try_claim` can reuse the session after handback, and its mapper state
    /// is left alone rather than torn down for a close that never happened.
    #[test]
    fn an_operator_who_adopts_first_keeps_the_session_alive_through_finish_turn() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let marker = dir.path().join("ticks");
        let sessions = PtyManager::new();
        let id = sessions.open(ticker(&marker)).expect("a live child");
        wait_for("the child to start writing", || ticks(&marker) > 0);

        assert!(sessions.set_control(&id, SessionControl::User));

        let exec = executor(sessions.clone());
        exec.workspace_context
            .lock()
            .expect("workspace context lock poisoned")
            .insert(id.clone(), (Some("cwd".to_string()), None, None));

        exec.finish_turn(&id, SessionClass::Bounded, true, RunTaskOrigin::Workflow);

        let row = sessions
            .row(&id)
            .expect("an operator-held session is never closed out from under them");
        assert!(row.state.is_running(), "the harness is still alive");
        assert!(
            !row.retained,
            "adoption, not retention — it is the operator's session on its own \
             terms, not a leftover flagged for read-and-release"
        );
        assert!(
            !row.busy,
            "must not be left permanently busy — that strands it from every \
             future claim, the exact leak this PR exists to close"
        );
        assert!(
            exec.workspace_context
                .lock()
                .expect("workspace context lock poisoned")
                .contains_key(&id),
            "mapper state must survive a close that was never attempted"
        );

        let before = ticks(&marker);
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            ticks(&marker) > before,
            "its child is genuinely still alive, not merely un-drawn"
        );

        // Hand back and confirm the session is genuinely reusable, not stuck
        // behind a busy flag nothing ever cleared.
        sessions.set_control(&id, SessionControl::Orchestrator);
        assert!(
            sessions
                .claim_idle("test", HarnessProvider::Codex)
                .is_some(),
            "a handed-back session must be claimable"
        );
        sessions.release(&id);
        sessions.close(&id);
    }

    /// `close_if_orchestrator`'s own ownership recheck, pinned directly and
    /// deterministically rather than via a live race: a session the operator
    /// already holds refuses the close and stays alive.
    #[test]
    fn close_if_orchestrator_refuses_a_session_the_operator_already_holds() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let marker = dir.path().join("ticks");
        let sessions = PtyManager::new();
        let id = sessions.open(ticker(&marker)).expect("a live child");
        wait_for("the child to start writing", || ticks(&marker) > 0);

        assert!(sessions.set_control(&id, SessionControl::User));
        assert_eq!(
            sessions.close_if_orchestrator(&id),
            CloseAttempt::UserOwned,
            "must refuse to close a session the operator holds"
        );

        let before = ticks(&marker);
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            ticks(&marker) > before,
            "the child must still be genuinely alive, not merely un-drawn"
        );

        sessions.close(&id);
    }

    /// The other half of the same recheck: a session still orchestrator-owned
    /// closes exactly as a plain `close` would.
    #[test]
    fn close_if_orchestrator_closes_a_session_still_orchestrator_owned() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let marker = dir.path().join("ticks");
        let sessions = PtyManager::new();
        let id = sessions.open(ticker(&marker)).expect("a live child");
        wait_for("the child to start writing", || ticks(&marker) > 0);

        assert_eq!(
            sessions.close_if_orchestrator(&id),
            CloseAttempt::Closed,
            "must close a session nobody has taken"
        );

        wait_for("the session to stop running", || {
            sessions.row(&id).is_some_and(|row| !row.state.is_running())
        });
        let before = ticks(&marker);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            ticks(&marker),
            before,
            "the harness process must actually be dead"
        );
    }

    /// The other ambiguity in `close_if_orchestrator`'s `false`: it also comes
    /// back when the session's handle is simply gone (the rail's per-frame
    /// sweep forgets any row whose child has exited, independent of whether
    /// `finish_turn` has run for it yet), not only when an operator holds it.
    /// Treating that as a takeover would be harmless for `settle_turn`/
    /// `release` — both are silent no-ops on an unknown id — but would leave
    /// its mapper state in `workspace_context` forever, for an id nothing will
    /// ever revisit.
    #[test]
    fn a_vanished_sessions_mapper_context_does_not_linger() {
        let sessions = PtyManager::new();
        let id = "never-opened".to_string();

        let exec = executor(sessions.clone());
        exec.workspace_context
            .lock()
            .expect("workspace context lock poisoned")
            .insert(id.clone(), (Some("cwd".to_string()), None, None));

        exec.finish_turn(&id, SessionClass::Bounded, true, RunTaskOrigin::Workflow);

        assert!(
            !exec
                .workspace_context
                .lock()
                .expect("workspace context lock poisoned")
                .contains_key(&id),
            "a vanished session's mapper state must not linger for an id \
             nothing will ever revisit"
        );
    }

    /// The interleaving codex actually described: not an id that was never
    /// opened, but a real child that exited on its own and whose row the
    /// rail's per-frame sweep (`sweep_finished_sessions`) has already
    /// forgotten by the time `finish_turn` gets around to processing that
    /// turn's completion. `forget` only waits on `is_running()`, not on
    /// `finish_turn` having run first, so this ordering is a real one, not a
    /// synthetic one — and its `close_if_orchestrator` outcome must read as
    /// [`CloseAttempt::Missing`], not as an operator takeover.
    #[test]
    fn a_forgotten_but_finished_sessions_mapper_context_does_not_linger() {
        let sessions = PtyManager::new();
        let id = sessions
            .open(short_lived())
            .expect("a child that exits immediately");

        wait_for("the session to stop running", || {
            sessions.row(&id).is_some_and(|row| !row.state.is_running())
        });
        assert!(
            sessions.forget(&id),
            "the sweep must be able to forget an exited, unheld session"
        );
        assert!(
            sessions.row(&id).is_none(),
            "and the handle is genuinely gone, not merely marked idle"
        );

        let exec = executor(sessions.clone());
        exec.workspace_context
            .lock()
            .expect("workspace context lock poisoned")
            .insert(id.clone(), (Some("cwd".to_string()), None, None));

        assert_eq!(
            sessions.close_if_orchestrator(&id),
            CloseAttempt::Missing,
            "a forgotten handle must read as missing, not as a takeover"
        );

        exec.finish_turn(&id, SessionClass::Bounded, true, RunTaskOrigin::Workflow);

        assert!(
            !exec
                .workspace_context
                .lock()
                .expect("workspace context lock poisoned")
                .contains_key(&id),
            "a forgotten-but-finished session's mapper state must not linger \
             for an id nothing will ever revisit"
        );
    }

    /// The other bug this PR's own race fix surfaced: a retained session's
    /// mapper state must survive `finish_turn` unconditionally. It used to —
    /// removal was gated on `!retain`. A refactor briefly added a second,
    /// independent `retains_workspace_context` check inside the `retain`
    /// branch, which tore the context down for every retained session anyway,
    /// because that function answers a different question (does an
    /// *unretained* session still have a running reader) that a `Bounded`,
    /// not-yet-`User` retained session never satisfies.
    #[test]
    fn a_retained_sessions_mapper_context_survives_finish_turn() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let marker = dir.path().join("ticks");
        let sessions = PtyManager::new();
        let id = sessions.open(ticker(&marker)).expect("a live child");
        wait_for("the child to start writing", || ticks(&marker) > 0);

        let exec = executor(sessions.clone());
        exec.workspace_context
            .lock()
            .expect("workspace context lock poisoned")
            .insert(id.clone(), (Some("cwd".to_string()), None, None));

        exec.finish_turn(
            &id,
            SessionClass::Bounded,
            true,
            RunTaskOrigin::DelegatedTask,
        );

        let row = sessions
            .row(&id)
            .expect("a retained session is still listed");
        assert!(
            row.retained,
            "the turn's origin has an operator in the loop"
        );
        assert!(
            exec.workspace_context
                .lock()
                .expect("workspace context lock poisoned")
                .contains_key(&id),
            "a retained session's mapper state must survive finish_turn"
        );

        sessions.close(&id);
    }
}
