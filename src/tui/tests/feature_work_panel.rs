//! End-to-end tests for the work surface: a worker's own todo list, plan,
//! sub-agents, and file edits reaching this terminal's screen.
//!
//! These drive the whole path a real run takes — structured harness events fold
//! into lanes, lanes render into the rail and the Work pane — because every
//! individual piece can be right while the screen still shows nothing.

use std::sync::Arc;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla_tui::ui::app::{App, TABS};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect()
}

/// The demo runtime on the Agents tab, with the cursor moved off the
/// orchestrator and onto the worker that is doing the work.
fn worker_app() -> App {
    let mut app = App::new(
        Arc::new(MockRuntime::demo()),
        LoadedConfig::defaults("medulla.tui.json".into()),
    );
    app.tab_index = TABS.iter().position(|t| *t == "Agents").unwrap();
    // Walk the rail the way an operator does: focus it, then step down onto the
    // worker lane under the orchestrator.
    for code in [KeyCode::Esc, KeyCode::Down] {
        app.on_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }
    app
}

#[test]
fn a_workers_todo_list_and_subagents_reach_the_screen() {
    let mut app = worker_app();
    let out = render(&mut app, 160, 44);
    assert!(
        out.contains("Splitting the session guard"),
        "the item the worker says it is on: {out}"
    );
    assert!(
        out.contains("review the extracted token"),
        "the sub-agent running underneath: {out}"
    );
    assert!(
        out.contains("tokens.rs"),
        "the file the worker rewrote: {out}"
    );
    assert!(
        out.contains("Split auth into focused modules"),
        "the goal it declared: {out}"
    );
}

#[test]
fn the_transcript_is_not_filled_with_the_raw_work_payloads() {
    // The work events are structured JSON. Rendered as transcript turns they
    // are a wall of braces; the panel is where that content is legible.
    let mut app = worker_app();
    let out = render(&mut app, 160, 44);
    assert!(
        !out.contains("\"status\":\"completed\""),
        "raw payloads must not reach the transcript: {out}"
    );
}

#[test]
fn the_rail_row_carries_the_progress_chip() {
    // The point of the chip is that an operator can see which machine to open
    // without opening any of them.
    let mut app = worker_app();
    let out = render(&mut app, 160, 44);
    assert!(out.contains("2/4"), "todo progress on the row: {out}");
    assert!(out.contains("⑂1"), "one sub-agent running: {out}");
}

#[test]
fn a_narrow_terminal_still_says_what_the_worker_is_on() {
    // Below the pane's width threshold the headline is the fallback, and it is
    // the one line that matters most.
    let mut app = worker_app();
    let out = render(&mut app, 90, 40);
    assert!(
        out.contains("work ·"),
        "the header headline stands in for the pane: {out}"
    );
    assert!(
        out.contains("Splitting the session guard"),
        "and it names the live item: {out}"
    );
}
