//! Tab and list-cursor navigation.

use crossterm::event::KeyCode;

use super::super::super::pty::PtyManager;
use super::helpers::{app_with, key, sh};

// ------------------------------------------------------------- navigation ---

#[test]
fn tab_and_number_keys_move_between_tabs() {
    let mut app = app_with(PtyManager::new(), None);
    assert_eq!(app.tab(), "Sessions");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.tab(), "Contacts");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.tab(), "Requests");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.tab(), "Sessions", "wraps");
    app.on_key(key(KeyCode::Char('3')));
    assert_eq!(app.tab(), "Requests");
}

#[test]
fn the_session_cursor_moves_and_clamps() {
    let sessions = PtyManager::new();
    sessions.open(sh("sleep 30", "a")).unwrap();
    sessions.open(sh("sleep 30", "b")).unwrap();
    let mut app = app_with(sessions.clone(), None);

    assert_eq!(app.selected_session().unwrap().label, "a");
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.selected_session().unwrap().label, "b");
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.selected_session().unwrap().label, "b", "clamps");
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.selected_session().unwrap().label, "a");
    sessions.shutdown();
}
