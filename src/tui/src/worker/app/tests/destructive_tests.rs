//! Confirmations guarding the actions that lose work.

use crossterm::event::KeyCode;

use super::super::super::pty::PtyManager;
use super::super::types::Confirm;
use super::helpers::{app_with, key, sh};

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
