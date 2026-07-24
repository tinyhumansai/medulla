//! Confirmations guarding the actions that lose work.

use crossterm::event::KeyCode;

use super::super::super::pty::PtyManager;
use super::super::types::Confirm;
use super::helpers::{app_with, key, render, sh, wait_for};

// --------------------------------------------------------- destructive ops ---

#[test]
fn killing_a_session_asks_first() {
    // A harness may be mid-edit; losing that to a stray keypress is not
    // acceptable.
    let sessions = PtyManager::new();
    let id = sessions.open(sh("sleep 30", "peer-1")).unwrap();
    let mut app = app_with(sessions.clone(), None);

    app.on_key(key(KeyCode::Char('K')));
    assert_eq!(app.confirm(), Some(&Confirm::CloseSession(id.clone())));
    assert!(
        sessions.row(&id).unwrap().state.is_running(),
        "not yet killed"
    );

    app.on_key(key(KeyCode::Char('y')));
    assert!(app.confirm().is_none());
    assert!(!sessions.row(&id).unwrap().state.is_running());
}

#[test]
fn anything_but_y_cancels_a_confirmation() {
    // A destructive action needs a deliberate yes, not merely "not Escape".
    let sessions = PtyManager::new();
    let id = sessions.open(sh("sleep 30", "peer-1")).unwrap();
    let mut app = app_with(sessions.clone(), None);

    app.on_key(key(KeyCode::Char('K')));
    app.on_key(key(KeyCode::Char('n')));
    assert!(app.confirm().is_none());
    assert!(sessions.row(&id).unwrap().state.is_running(), "still alive");
    assert!(app.status().contains("Cancelled"));
    sessions.shutdown();
}

#[test]
fn a_running_session_cannot_be_dropped() {
    let sessions = PtyManager::new();
    sessions.open(sh("sleep 30", "peer-1")).unwrap();
    let mut app = app_with(sessions.clone(), None);

    app.on_key(key(KeyCode::Char('d')));
    assert_eq!(app.session_rows().len(), 1);
    assert!(app.status().contains("press K to kill it first"));
    sessions.shutdown();
}

#[test]
fn dropping_an_exited_session_forgets_it_without_asking() {
    // A session that has already exited holds no work to lose, so `d` removes it
    // from the list straight away — no confirmation.
    let sessions = PtyManager::new();
    let id = sessions.open(sh("true", "peer-done")).unwrap();
    wait_for("the session to exit", || {
        !sessions.row(&id).unwrap().state.is_running()
    });
    let mut app = app_with(sessions.clone(), None);

    app.on_key(key(KeyCode::Char('d')));
    assert!(
        app.confirm().is_none(),
        "dropping an exited session does not ask"
    );
    assert!(
        app.session_rows().is_empty(),
        "the exited session is gone from the list"
    );
    assert!(
        app.status().contains("Dropped peer-done"),
        "got {:?}",
        app.status()
    );
    sessions.shutdown();
}

#[test]
fn killing_an_already_exited_session_says_so_rather_than_asking() {
    let sessions = PtyManager::new();
    let id = sessions.open(sh("true", "peer-done")).unwrap();
    wait_for("the session to exit", || {
        !sessions.row(&id).unwrap().state.is_running()
    });
    let mut app = app_with(sessions.clone(), None);

    app.on_key(key(KeyCode::Char('K')));
    assert!(
        app.confirm().is_none(),
        "nothing running to confirm killing"
    );
    assert!(
        app.status().contains("already exited"),
        "got {:?}",
        app.status()
    );
    sessions.shutdown();
}

#[test]
fn a_pending_confirmation_swaps_in_its_own_key_hints() {
    // While a destructive action is armed the status line must offer the
    // confirm/cancel keys, not the tab's normal hints — otherwise the operator
    // has no on-screen prompt for the "y" the action is waiting on.
    let sessions = PtyManager::new();
    sessions.open(sh("sleep 30", "peer-1")).unwrap();
    let mut app = app_with(sessions.clone(), None);

    app.on_key(key(KeyCode::Char('K')));
    assert!(app.confirm().is_some(), "the kill is armed");
    let out = render(&mut app, 110, 16);
    assert!(
        out.contains("y confirm"),
        "the confirmation prompt replaces the tab hints: {out}"
    );
    sessions.shutdown();
}

#[test]
fn kill_and_drop_report_when_there_is_no_session_to_act_on() {
    let mut app = app_with(PtyManager::new(), None);

    app.on_key(key(KeyCode::Char('K')));
    assert_eq!(app.status(), "No session selected");

    app.on_key(key(KeyCode::Char('d')));
    assert_eq!(app.status(), "No session selected");
}
