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
fn operator_hold_reports_the_session_holding_a_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let manager = PtyManager::new();
    let id = manager.open(user_sh_in("sleep 30", dir.path())).unwrap();
    wait_for("session running", || {
        manager.row(&id).is_some_and(|r| r.state.is_running())
    });

    let held = manager
        .operator_hold(&dir.path().to_string_lossy())
        .expect("a workspace the operator is working in must report its hold");
    assert_eq!(held.id, id);

    manager.close(&id);
}

#[test]
fn operator_hold_ignores_a_session_the_orchestrator_holds() {
    // The whole point of the check: it must gate on *who holds it*, not on
    // "is there a harness here". An orchestrator session in a folder is not a
    // reason to refuse the orchestrator work in that folder.
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

    assert_eq!(manager.operator_hold(&dir.path().to_string_lossy()), None);

    manager.close(&id);
}

#[test]
fn operator_hold_ignores_a_workspace_nobody_is_in() {
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let manager = PtyManager::new();
    let id = manager.open(user_sh_in("sleep 30", dir.path())).unwrap();
    wait_for("session running", || {
        manager.row(&id).is_some_and(|r| r.state.is_running())
    });

    assert_eq!(
        manager.operator_hold(&other.path().to_string_lossy()),
        None,
        "a hold must not leak across workspaces"
    );

    manager.close(&id);
}

#[test]
fn operator_hold_releases_when_the_session_exits() {
    // A dead harness cannot be handed back, so a hold that outlived its process
    // would wedge the workspace shut with no way to reopen it.
    let dir = tempfile::tempdir().unwrap();
    let manager = PtyManager::new();
    let id = manager.open(user_sh_in("true", dir.path())).unwrap();
    wait_for("session exited", || {
        manager.row(&id).is_some_and(|r| !r.state.is_running())
    });

    assert_eq!(manager.operator_hold(&dir.path().to_string_lossy()), None);

    manager.close(&id);
}

#[test]
fn operator_hold_matches_the_same_directory_written_two_ways() {
    // The two sides arrive by different routes — an operator-spawned harness had
    // its path expanded, a task frame's cwd is verbatim — so the hold has to
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
        assert!(
            manager.operator_hold(&spelling).is_some(),
            "the hold was lost when the path was written as {spelling}"
        );
    }

    manager.close(&id);
}

// ------------------------------------------------------------ provenance ---

#[test]
fn taking_a_dispatched_session_and_handing_it_back_never_changes_its_origin() {
    // The distinction this whole field exists for. Control is a question about
    // *now* and moves with every takeover; origin is a fact about how the
    // session was born and moves never. A rail that read them as one thing would
    // lose a dispatched session out of its agent's group the moment an operator
    // pressed ctrl-g on it.
    let manager = PtyManager::new();
    let id = manager.open(sh("sleep 30")).unwrap();
    let row = manager.row(&id).unwrap();
    assert!(row.origin.is_orchestrator());
    assert_eq!(row.control, SessionControl::Orchestrator);

    assert!(manager.set_control(&id, SessionControl::User));
    let taken = manager.row(&id).unwrap();
    assert_eq!(taken.control, SessionControl::User, "the operator holds it");
    assert!(
        taken.origin.is_orchestrator(),
        "holding a session is not having started it"
    );

    assert!(manager.set_control(&id, SessionControl::Orchestrator));
    assert!(
        manager.row(&id).unwrap().origin.is_orchestrator(),
        "handing it back changes control, and only control"
    );

    manager.close(&id);
}

#[test]
fn handing_an_operator_started_session_to_the_orchestrator_keeps_it_user_originated() {
    // The mirror case, and the one that makes the two axes visibly independent:
    // the session becomes dispatchable — a control fact — while still being one
    // a person started.
    let manager = PtyManager::new();
    let id = manager.open(user_sh("sleep 30")).unwrap();
    wait_for("session running", || {
        manager.row(&id).is_some_and(|r| r.state.is_running())
    });

    assert!(manager.set_control(&id, SessionControl::Orchestrator));
    let row = manager.row(&id).unwrap();
    assert!(row.origin.is_user(), "origin is fixed at birth");
    assert_eq!(row.control, SessionControl::Orchestrator);
    assert!(
        manager
            .claim_idle("test", HarnessProvider::Codex)
            .is_some_and(|claimed| claimed.id == id),
        "a handed-over session is dispatchable however it was started"
    );

    manager.close(&id);
}

#[test]
fn a_session_name_round_trips_and_a_blank_one_clears_it() {
    // The name is the operator's label for a session they spun up; it is display
    // identity only, and it never touches provenance or control.
    let manager = PtyManager::new();
    let id = manager
        .open(LaunchSpec {
            name: Some("debug login".to_string()),
            ..user_sh("sleep 30")
        })
        .unwrap();
    assert_eq!(
        manager.row(&id).unwrap().name.as_deref(),
        Some("debug login")
    );

    assert!(manager.set_name(&id, Some("chasing the 500".to_string())));
    let renamed = manager.row(&id).unwrap();
    assert_eq!(renamed.name.as_deref(), Some("chasing the 500"));
    assert!(renamed.origin.is_user(), "renaming is not re-parenting");
    assert_eq!(renamed.control, SessionControl::User);

    // Blank is not a name: storing it would render as a gap in the rail.
    assert!(manager.set_name(&id, Some("   ".to_string())));
    assert_eq!(manager.row(&id).unwrap().name, None);
    assert!(manager.set_name(&id, Some("back".to_string())));
    assert!(manager.set_name(&id, None));
    assert_eq!(manager.row(&id).unwrap().name, None);

    assert!(!manager.set_name("w_nope", Some("ghost".to_string())));

    manager.close(&id);
}

#[test]
fn a_dispatched_session_is_born_unnamed() {
    // Nothing to name it: the UI labels an orchestrator-originated session from
    // the task it was created for, which is why this stays `None` rather than
    // getting a synthetic string here.
    let manager = PtyManager::new();
    let id = manager.open(sh("sleep 30")).unwrap();
    assert_eq!(manager.row(&id).unwrap().name, None);
    manager.close(&id);
}
