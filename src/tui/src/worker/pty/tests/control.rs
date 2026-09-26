//! Who holds a session, and what that changes about dispatch.
//!
//! The gate lives in one clause of [`PtyManager::claim_idle`], so these tests
//! are about that clause: a session the operator holds must be invisible to
//! dispatch no matter how idle it looks, and handing it back must make it
//! visible again.

use super::*;

use medulla::protocol::HarnessProvider;

/// A spec for a session the operator asked for, rather than a task frame.
fn user_sh(script: &str) -> LaunchSpec {
    LaunchSpec {
        control: SessionControl::User,
        origin: crate::worker::pty::SessionOrigin::User,
        name: None,
        ..sh(script)
    }
}

#[test]
fn a_session_the_operator_holds_is_never_dispatched_into() {
    let manager = PtyManager::new();
    let id = manager.open(user_sh("sleep 30")).unwrap();
    wait_for("session running", || {
        manager.row(&id).is_some_and(|r| r.state.is_running())
    });

    // Idle, running, right label, right provider — everything `claim_idle` looks
    // for except who holds it.
    assert_eq!(
        manager.claim_idle("test", HarnessProvider::Codex),
        None,
        "an operator-held session was offered to the orchestrator"
    );

    manager.close(&id);
}

#[test]
fn handing_a_session_back_makes_it_claimable() {
    let manager = PtyManager::new();
    let id = manager.open(user_sh("sleep 30")).unwrap();
    wait_for("session running", || {
        manager.row(&id).is_some_and(|r| r.state.is_running())
    });
    assert!(manager.claim_idle("test", HarnessProvider::Codex).is_none());

    assert!(manager.set_control(&id, SessionControl::Orchestrator));
    let claimed = manager
        .claim_idle("test", HarnessProvider::Codex)
        .expect("a handed-back session is the orchestrator's to use");
    assert_eq!(claimed.id, id);

    manager.close(&id);
}

#[test]
fn a_retained_session_is_never_dispatched_into() {
    // A retained session is a finished task's, kept on screen. Reusing it would
    // put the next task's prompt under a conversation that has already
    // concluded — and would do it to a transcript the operator is reading.
    let manager = PtyManager::new();
    let id = manager.open(sh("sleep 30")).unwrap();
    wait_for("session running", || {
        manager.row(&id).is_some_and(|r| r.state.is_running())
    });
    // A task-opened session is born busy — it exists for the turn about to run.
    // Freeing it first is what makes the assertion below about retention rather
    // than about the claim it was already holding.
    manager.release(&id);
    assert!(
        manager.claim_idle("test", HarnessProvider::Codex).is_some(),
        "precondition: this session is claimable before it is retained"
    );
    manager.release(&id);

    assert!(manager.retain(&id));
    assert_eq!(
        manager.claim_idle("test", HarnessProvider::Codex),
        None,
        "a retained session was offered to the orchestrator"
    );

    manager.close(&id);
}

#[test]
fn a_retained_session_says_so_on_its_row() {
    // The rail decides what to draw from the row, so retention that never
    // reaches one is retention nothing can show.
    let manager = PtyManager::new();
    let id = manager.open(sh("sleep 30")).unwrap();
    wait_for("session running", || {
        manager.row(&id).is_some_and(|r| r.state.is_running())
    });
    assert!(!manager.row(&id).expect("row").retained);

    manager.retain(&id);
    let row = manager.row(&id).expect("row");
    assert!(row.retained);
    assert_eq!(
        row.control,
        SessionControl::Orchestrator,
        "retention must not read as a takeover — `checkout_writer` counts \
         user-held sessions as holding the checkout, and one that never lets go \
         would queue every task dispatched into that directory afterwards"
    );

    manager.close(&id);
}

#[test]
fn taking_a_retained_session_makes_it_the_operators() {
    // Retention ends the moment someone takes the session: it stops being the
    // leftover screen of finished work and becomes a place a person is typing.
    let manager = PtyManager::new();
    let id = manager.open(sh("sleep 30")).unwrap();
    wait_for("session running", || {
        manager.row(&id).is_some_and(|r| r.state.is_running())
    });
    manager.retain(&id);

    assert!(manager.set_control(&id, SessionControl::User));
    let row = manager.row(&id).expect("row");
    assert!(!row.retained, "taking it should clear the retention");
    assert_eq!(row.control, SessionControl::User);

    manager.close(&id);
}

#[test]
fn retaining_a_session_that_is_gone_is_not_an_error() {
    // Same contract as `close`: the caller cannot tell "retained" from "already
    // gone" by anything but the return value.
    let manager = PtyManager::new();
    assert!(!manager.retain("w_missing"));
}

#[test]
fn handed_back_operator_session_adopts_the_first_real_conversation() {
    let manager = PtyManager::new();
    let mut spec = user_sh("sleep 30");
    spec.label = "you:codex".to_string();
    let id = manager.open(spec).unwrap();
    wait_for("session running", || {
        manager.row(&id).is_some_and(|r| r.state.is_running())
    });

    assert!(manager.set_control(&id, SessionControl::Orchestrator));
    let claimed = manager
        .claim_idle("peer@example", HarnessProvider::Codex)
        .expect("a handed-back session must be eligible for real dispatch");
    assert_eq!(claimed.id, id);
    assert_eq!(claimed.label, "peer@example");

    manager.release(&id);
    assert!(
        manager
            .claim_idle("another-peer", HarnessProvider::Codex)
            .is_none(),
        "adoption must not turn the session into a cross-conversation pool"
    );
    assert!(manager
        .claim_idle("peer@example", HarnessProvider::Codex)
        .is_some());

    manager.close(&id);
}

#[test]
fn taking_over_a_running_session_stops_further_dispatch() {
    let manager = PtyManager::new();
    // Opened the way a task frame opens one: orchestrator-held.
    let id = manager.open(sh("sleep 30")).unwrap();
    wait_for("session running", || {
        manager.row(&id).is_some_and(|r| r.state.is_running())
    });
    // The launch claims it; releasing is what the executor does when its turn
    // ends, and it is that idle window a second task would reuse.
    manager.release(&id);
    assert!(
        manager.claim_idle("test", HarnessProvider::Codex).is_some(),
        "an idle orchestrator session should be reusable"
    );
    manager.release(&id);

    assert!(manager.set_control(&id, SessionControl::User));
    assert_eq!(
        manager.claim_idle("test", HarnessProvider::Codex),
        None,
        "taking over a harness must stop the next task landing in it"
    );

    manager.close(&id);
}

#[test]
fn an_operator_spawned_session_opens_idle() {
    // A task-frame session is claimed at open because a turn is about to run in
    // it. Nothing is about to run in this one — it is sitting at a prompt — and
    // a rail that called it busy would be lying.
    let manager = PtyManager::new();
    let id = manager.open(user_sh("sleep 30")).unwrap();
    let row = manager.row(&id).unwrap();
    assert!(!row.busy, "an operator-spawned session opens idle");
    assert!(row.origin.is_user());
    assert_eq!(row.control, SessionControl::User);
    manager.close(&id);
}

#[test]
fn control_of_an_unknown_session_is_not_reported_or_settable() {
    let manager = PtyManager::new();
    assert_eq!(manager.control("w_nope"), None);
    assert!(!manager.set_control("w_nope", SessionControl::User));
}

#[test]
fn handover_leaves_a_running_turn_marked_busy() {
    // `busy` and `control` answer different questions. A session taken over
    // mid-turn is still running that turn, and clearing the flag on handback
    // would advertise it as free while it finished someone else's work.
    let manager = PtyManager::new();
    let id = manager.open(sh("sleep 30")).unwrap();
    assert!(manager.row(&id).unwrap().busy, "a task launch claims it");

    assert!(manager.set_control(&id, SessionControl::User));
    assert!(manager.row(&id).unwrap().busy);
    assert!(manager.set_control(&id, SessionControl::Orchestrator));
    assert!(
        manager.row(&id).unwrap().busy,
        "handback must not free a session whose turn is still running"
    );

    manager.close(&id);
}

/// A session the operator holds, running in `cwd`.
fn user_sh_in(script: &str, cwd: &std::path::Path) -> LaunchSpec {
    LaunchSpec {
        cwd: cwd.to_string_lossy().into_owned(),
        ..user_sh(script)
    }
}

#[test]
fn sessions_in_reports_what_is_running_in_a_directory_and_who_holds_it() {
    // A neutral question, and the replacement for the old `operator_hold(cwd)`.
    // That one asked "is this *workspace* held" — an artifact of the model where
    // an agent had one implicit session. A hold is on a session; a directory
    // only ever has sessions *in* it, and what that implies is the strategy's
    // business, not control's.
    let dir = tempfile::tempdir().unwrap();
    let manager = PtyManager::new();
    let id = manager.open(user_sh_in("sleep 30", dir.path())).unwrap();
    wait_for("session running", || {
        manager.row(&id).is_some_and(|r| r.state.is_running())
    });

    let running = manager.sessions_in(&dir.path().to_string_lossy());
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].id, id);
    assert_eq!(
        running[0].control,
        SessionControl::User,
        "the answer carries who holds each session rather than filtering on it"
    );

    manager.close(&id);
}

#[test]
fn sessions_in_lists_an_orchestrator_session_too() {
    // The old query dropped these, because it was really asking "may anything
    // start here". This one reports them, so the caller applying the checkout's
    // one-writer rule can see every writer rather than only the human one — the
    // seam F3 needs to serialize orchestrator sessions without a new query.
    let dir = tempfile::tempdir().unwrap();
    let manager = PtyManager::new();
    let spec = LaunchSpec {
        cwd: dir.path().to_string_lossy().into_owned(),
        ..sh("sleep 30")
    };
    let id = manager.open(spec).unwrap();
    wait_for("session running", || {
        manager.row(&id).is_some_and(|r| r.state.is_running())
    });

    let running = manager.sessions_in(&dir.path().to_string_lossy());
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].control, SessionControl::Orchestrator);

    manager.close(&id);
}

#[test]
fn sessions_in_does_not_leak_across_directories() {
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let manager = PtyManager::new();
    let id = manager.open(user_sh_in("sleep 30", dir.path())).unwrap();
    wait_for("session running", || {
        manager.row(&id).is_some_and(|r| r.state.is_running())
    });

    assert!(
        manager
            .sessions_in(&other.path().to_string_lossy())
            .is_empty(),
        "a session must not be reported in a directory it is not running in"
    );

    manager.close(&id);
}

#[test]
fn sessions_in_forgets_a_session_that_exited() {
    // A dead harness writes nothing, so counting it as a writer would wedge the
    // checkout shut with no way to reopen it.
    let dir = tempfile::tempdir().unwrap();
    let manager = PtyManager::new();
    let id = manager.open(user_sh_in("true", dir.path())).unwrap();
    wait_for("session exited", || {
        manager.row(&id).is_some_and(|r| !r.state.is_running())
    });

    assert!(manager
        .sessions_in(&dir.path().to_string_lossy())
        .is_empty());

    manager.close(&id);
}

#[test]
fn sessions_in_matches_the_same_directory_written_two_ways() {
    // The two sides arrive by different routes — an operator-spawned harness had
    // its path expanded, a task frame's cwd is verbatim — so the match has to
    // survive a trailing slash and a symlinked path. Exclusivity that can be
    // defeated by spelling is not exclusivity.
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("workspace");
    std::fs::create_dir(&real).unwrap();
    let link = dir.path().join("link-to-workspace");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let manager = PtyManager::new();
    let id = manager.open(user_sh_in("sleep 30", &real)).unwrap();
    wait_for("session running", || {
        manager.row(&id).is_some_and(|r| r.state.is_running())
    });

    for spelling in [
        real.to_string_lossy().into_owned(),
        format!("{}/", real.to_string_lossy()),
        link.to_string_lossy().into_owned(),
    ] {
        assert_eq!(
            manager.sessions_in(&spelling).len(),
            1,
            "the session was lost when the path was written as {spelling}"
        );
    }

    manager.close(&id);
}
