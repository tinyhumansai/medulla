//! `/harness <provider> [path]`: starting a named provider without the picker,
//! and the two ways that can be asked for something impossible.

use crate::helpers::*;
use crossterm::event::KeyCode;
use medulla_tui::worker::pty::{PtyManager, SessionControl};

#[test]
fn the_slash_command_starts_a_named_provider_directly() {
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());

    type_str(&mut app, "/harness codex /");
    let _ = app.on_event(key(KeyCode::Enter));
    wait_for("the harness to open", || sessions.rows().len() == 1);

    let row = sessions.rows().remove(0);
    assert_eq!(row.control, SessionControl::User);
    assert_eq!(row.cwd, "/");

    sessions.shutdown();
}

#[test]
fn a_misspelled_provider_reports_usage_rather_than_starting_one() {
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());

    type_str(&mut app, "/harness claud");
    let _ = app.on_event(key(KeyCode::Enter));

    assert!(app.status().contains("Usage: /session"), "{}", app.status());
    assert!(sessions.rows().is_empty(), "nothing should have started");
}

#[test]
fn starting_a_harness_in_a_missing_directory_says_so() {
    let sessions = PtyManager::new();
    let mut app = app_with_harnesses(sessions.clone());

    type_str(&mut app, "/harness codex /no/such/place");
    let _ = app.on_event(key(KeyCode::Enter));

    assert!(
        app.status().contains("not a directory"),
        "a bad path is named, not swallowed: {}",
        app.status()
    );
    assert!(sessions.rows().is_empty());
}
