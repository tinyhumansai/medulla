//! Pointer-input and mouse-capture tests for the worker TUI.

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use super::super::super::pty::PtyManager;
use super::super::types::{ExecutionMode, SetupStep, WorkerCmd, TAB_MASTER};
use super::helpers::{app_at_setup, app_with, render, render_lines};

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
    let mut app = app_with(PtyManager::new());
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
fn a_pasted_address_lands_in_the_prompt_instead_of_being_dropped() {
    let mut app = app_with(PtyManager::new());
    app.set_tab(TAB_MASTER);
    // `a` opens the pairing prompt; the address is pasted, not typed.
    app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char('a'),
        KeyModifiers::NONE,
    )));

    assert!(
        app.on_event(Event::Paste("0xabc\r\n".into())).is_none(),
        "a paste never submits"
    );
    assert_eq!(
        app.prompt.as_ref().map(|p| p.draft.text.as_str()),
        Some("0xabc "),
        "the payload sits in the prompt, flattened to one line"
    );

    // Enter is what submits, and the flattened trailing newline trims away.
    let command = app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )));
    assert!(
        matches!(command, Some(WorkerCmd::ConnectMaster(ref a)) if a == "0xabc"),
        "pasted address submitted: {command:?}"
    );
}

#[test]
fn a_paste_with_no_prompt_open_is_ignored() {
    let mut app = app_with(PtyManager::new());

    assert!(app.on_event(Event::Paste("stray".into())).is_none());
    assert!(app.prompt.is_none(), "no prompt was conjured");
}

#[test]
fn repeated_ctrl_o_does_not_flip_mouse_capture() {
    let mut app = app_with(PtyManager::new());

    app.on_event(Event::Key(KeyEvent::new_with_kind(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
        KeyEventKind::Repeat,
    )));

    assert!(app.mouse_capture());
}

#[test]
fn buffered_mouse_events_are_ignored_after_capture_is_released() {
    let mut app = app_with(PtyManager::new());
    let _ = render(&mut app, 100, 30);
    app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));

    app.on_event(click(37, 1));

    assert_eq!(app.tab(), "Agents");
}

#[test]
fn released_mouse_hint_offers_recapture() {
    let mut app = app_with(PtyManager::new());
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
    let mut app = app_at_setup(PtyManager::new());
    let _ = render(&mut app, 100, 30);

    let command = app.on_event(click(5, 6));

    assert!(command.is_none());
    assert_eq!(app.mode(), Some(ExecutionMode::Interactive));
    assert_eq!(app.setup_step(), SetupStep::Harness);
}

#[test]
fn setup_options_stay_on_clickable_rows_in_a_narrow_terminal() {
    let mut app = app_at_setup(PtyManager::new());
    let lines = render_lines(&mut app, 30, 20);

    assert!(lines[6].contains("Interactive"), "{}", lines[6]);
    assert!(lines[7].contains("Headless"), "{}", lines[7]);
    app.on_event(click(5, 6));
    assert_eq!(app.mode(), Some(ExecutionMode::Interactive));
}

#[test]
fn tab_labels_can_be_clicked() {
    let mut app = app_with(PtyManager::new());
    let _ = render(&mut app, 100, 30);

    // Row 2: the header is two lines (identity, then what this worker serves).
    app.on_event(click(28, 2));

    assert_eq!(app.tab(), "Workspaces");
}

#[test]
fn narrow_master_rows_stay_aligned_with_their_hitboxes() {
    let mut app = app_with(PtyManager::new());
    app.add_master("a-very-long-master-identifier-that-would-wrap".into(), None);
    app.add_master("bob".into(), None);
    app.set_tab(1);

    let lines = render_lines(&mut app, 40, 12);

    assert!(lines[5].contains("bob"), "{}", lines[5]);
    app.on_event(click(3, 5));
    assert_eq!(app.selected_master_address().as_deref(), Some("bob"));
}
