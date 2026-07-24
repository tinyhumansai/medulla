//! Pointer-input and mouse-capture tests for the worker TUI.

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use super::super::super::pty::PtyManager;
use super::super::types::{ExecutionMode, SetupStep};
use super::helpers::{app_at_setup, app_with, desk_with, desk_with_contacts, render, render_lines};

/// Build a synthetic left-click at a terminal cell.
fn click(column: u16, row: u16) -> Event {
    Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

#[test]
fn ctrl_o_releases_and_recaptures_the_mouse_for_native_copy() {
    let mut app = app_with(PtyManager::new(), None);
    assert!(app.mouse_capture());

    app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
    assert!(!app.mouse_capture());
    assert!(app.status().contains("native"));

    app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
    assert!(app.mouse_capture());
    assert!(app.status().contains("click"));
}

#[test]
fn repeated_ctrl_o_does_not_flip_mouse_capture() {
    let mut app = app_with(PtyManager::new(), None);

    app.on_event(Event::Key(KeyEvent::new_with_kind(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
        KeyEventKind::Repeat,
    )));

    assert!(app.mouse_capture());
}

#[tokio::test]
async fn repeated_decision_key_does_not_act_on_the_next_request() {
    let desk = desk_with(&["alice", "bob"]).await;
    let mut app = app_with(PtyManager::new(), Some(desk));
    app.set_tab(2);

    let command = app.on_event(Event::Key(KeyEvent::new_with_kind(
        KeyCode::Char('a'),
        KeyModifiers::NONE,
        KeyEventKind::Repeat,
    )));

    assert!(command.is_none());
    assert_eq!(app.selected_request().unwrap().agent_id, "alice");
}

#[test]
fn buffered_mouse_events_are_ignored_after_capture_is_released() {
    let mut app = app_with(PtyManager::new(), None);
    let _ = render(&mut app, 100, 30);
    app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));

    app.on_event(click(27, 1));

    assert_eq!(app.tab(), "Sessions");
}

#[test]
fn released_mouse_hint_offers_recapture() {
    let mut app = app_with(PtyManager::new(), None);
    app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
    app.set_status("Ready");

    let output = render(&mut app, 120, 20);

    assert!(output.contains("^O enable mouse"), "{output}");
}

#[test]
fn setup_options_can_be_clicked() {
    let mut app = app_at_setup(PtyManager::new(), None);
    let _ = render(&mut app, 100, 30);

    let command = app.on_event(click(5, 7));

    assert!(command.is_none());
    assert_eq!(app.mode(), Some(ExecutionMode::Interactive));
    assert_eq!(app.setup_step(), SetupStep::Harness);
}

#[test]
fn setup_options_stay_on_clickable_rows_in_a_narrow_terminal() {
    let mut app = app_at_setup(PtyManager::new(), None);
    let lines = render_lines(&mut app, 30, 20);

    assert!(lines[6].contains("Headless"), "{}", lines[6]);
    assert!(lines[7].contains("Interactive"), "{}", lines[7]);
    app.on_event(click(5, 7));
    assert_eq!(app.mode(), Some(ExecutionMode::Interactive));
}

#[test]
fn tab_labels_can_be_clicked() {
    let mut app = app_with(PtyManager::new(), None);
    let _ = render(&mut app, 100, 30);

    app.on_event(click(27, 1));

    assert_eq!(app.tab(), "Requests");
}

#[tokio::test]
async fn list_rows_can_be_clicked() {
    let desk = desk_with(&["alice", "bob"]).await;
    let mut app = app_with(PtyManager::new(), Some(desk));
    app.set_tab(2);
    let _ = render(&mut app, 100, 30);

    app.on_event(click(3, 4));

    assert_eq!(app.selected_request().unwrap().agent_id, "bob");
}

#[tokio::test]
async fn narrow_contact_rows_stay_aligned_with_their_hitboxes() {
    let desk = desk_with_contacts(
        &[],
        &["a-very-long-contact-identifier-that-would-wrap", "bob"],
    )
    .await;
    let mut app = app_with(PtyManager::new(), Some(desk));
    app.set_tab(1);

    let lines = render_lines(&mut app, 24, 12);

    assert!(lines[4].contains("bob"), "{}", lines[4]);
    app.on_event(click(3, 4));
    assert_eq!(app.selected_contact().unwrap().agent_id, "bob");
}
