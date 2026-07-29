//! Who holds a session, and what that changes about dispatch.
//!
//! The gate lives in one clause of [`PtyManager::claim_idle`], so these tests
//! are about that clause: a session the operator holds must be invisible to
//! dispatch no matter how idle it looks, and handing it back must make it
//! visible again.

use super::*;

use medulla::tinyplace::HarnessProvider;

/// A spec for a session the operator asked for, rather than a task frame.
fn user_sh(script: &str) -> LaunchSpec {
    LaunchSpec {
        control: HarnessControl::User,
        user_spawned: true,
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

    assert!(manager.set_control(&id, HarnessControl::Orchestrator));
    let claimed = manager
        .claim_idle("test", HarnessProvider::Codex)
        .expect("a handed-back session is the orchestrator's to use");
    assert_eq!(claimed.id, id);

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

    assert!(manager.set_control(&id, HarnessControl::User));
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
    assert!(row.user_spawned);
    assert_eq!(row.control, HarnessControl::User);
    manager.close(&id);
}

#[test]
fn control_of_an_unknown_session_is_not_reported_or_settable() {
    let manager = PtyManager::new();
    assert_eq!(manager.control("w_nope"), None);
    assert!(!manager.set_control("w_nope", HarnessControl::User));
}

#[test]
fn handover_leaves_a_running_turn_marked_busy() {
    // `busy` and `control` answer different questions. A session taken over
    // mid-turn is still running that turn, and clearing the flag on handback
    // would advertise it as free while it finished someone else's work.
    let manager = PtyManager::new();
    let id = manager.open(sh("sleep 30")).unwrap();
    assert!(manager.row(&id).unwrap().busy, "a task launch claims it");

    assert!(manager.set_control(&id, HarnessControl::User));
    assert!(manager.row(&id).unwrap().busy);
    assert!(manager.set_control(&id, HarnessControl::Orchestrator));
    assert!(
        manager.row(&id).unwrap().busy,
        "handback must not free a session whose turn is still running"
    );

    manager.close(&id);
}
