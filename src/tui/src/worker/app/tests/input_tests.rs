//! Pointer-input and mouse-capture tests for the worker TUI.

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use super::super::super::pty::PtyManager;
use super::super::types::{ExecutionMode, SetupStep};
use super::helpers::{app_at_setup, app_with, render};

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
fn setup_options_can_be_clicked() {
    let mut app = app_at_setup(PtyManager::new(), None);
    let _ = render(&mut app, 100, 30);

    let command = app.on_event(click(5, 7));

    assert!(command.is_none());
    assert_eq!(app.mode(), Some(ExecutionMode::Interactive));
    assert_eq!(app.setup_step(), SetupStep::Harness);
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
    let desk = super::helpers::desk_with(&["alice", "bob"]).await;
    let mut app = app_with(PtyManager::new(), Some(desk));
    app.set_tab(2);
    let _ = render(&mut app, 100, 30);

    app.on_event(click(3, 4));

    assert_eq!(app.selected_request().unwrap().agent_id, "bob");
}
