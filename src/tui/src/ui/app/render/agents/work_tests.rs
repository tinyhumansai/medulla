//! Tests for the Work pane: what it shows, when it opens, and what it does with
//! a terminal too narrow to hold it.

use std::sync::Arc;

use medulla::config::LoadedConfig;
use medulla::harness_work::{kinds, WorkFold, WorkSnapshot};
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use serde_json::json;

use crate::ui::app::App;

fn app() -> App {
    let rt: Arc<dyn Runtime> = Arc::new(MockRuntime::empty());
    App::new(rt, LoadedConfig::defaults("medulla.tui.json".into()))
}

/// A snapshot with a todo list, a running sub-agent, and a touched file — the
/// three things the pane exists to show.
fn busy_snapshot() -> WorkSnapshot {
    let mut fold = WorkFold::new();
    fold.apply(
        kinds::PLAN_UPDATE,
        &json!({ "goal": "make the fleet legible" }),
        1,
    );
    fold.apply(
        kinds::TODO_UPDATE,
        &json!({ "todos": [
            { "content": "survey the harnesses", "status": "completed" },
            { "content": "fold their work", "status": "in_progress" },
        ]}),
        2,
    );
    fold.apply(
        kinds::SUBAGENT_START,
        &json!({ "call_id": "s1", "description": "review the diff" }),
        3,
    );
    fold.apply(
        kinds::FILE_CHANGE,
        &json!({ "path": "src/sdk/src/lib.rs", "change": "modified" }),
        4,
    );
    fold.into_snapshot()
}

/// Draw just the work pane and read the buffer back as text.
fn draw(work: &WorkSnapshot, width: u16, height: u16) -> String {
    let mut app = app();
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|f| {
            app.draw_agents_work(f, Rect::new(0, 0, width, height), work);
        })
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect()
}

#[test]
fn the_pane_shows_the_goal_todos_subagents_and_files() {
    let out = draw(&busy_snapshot(), 40, 24);
    assert!(out.contains("make the fleet legible"), "the goal: {out}");
    assert!(out.contains("fold their work"), "the live todo: {out}");
    assert!(out.contains("review the diff"), "the sub-agent: {out}");
    assert!(out.contains("lib.rs"), "the touched file: {out}");
    assert!(out.contains("Work · 1/2"), "progress in the title: {out}");
}

#[test]
fn an_overflowing_panel_keeps_its_head_and_says_what_it_dropped() {
    // The goal and the live todo are why the pane was opened; the file list is
    // not worth losing them for.
    let out = draw(&busy_snapshot(), 40, 8);
    assert!(
        out.contains("make the fleet legible"),
        "the goal survives: {out}"
    );
    assert!(out.contains("more line(s)"), "the overflow is named: {out}");
}

#[test]
fn a_narrow_terminal_keeps_the_transcript_whole() {
    // Below the width threshold the pane is not laid out at all — the header
    // headline carries the one line that matters instead.
    let mut app = app();
    let selection = app.agents_selection();
    let narrow = app.agents_panes(Rect::new(0, 0, 80, 30), &selection);
    assert!(narrow.work.is_none());
}

#[test]
fn the_pane_stays_shut_on_a_selection_with_nothing_to_show() {
    let mut app = app();
    let selection = app.agents_selection();
    // The mock runtime's lanes carry no work, so a wide terminal still opens
    // nothing: an empty panel is worse than no panel.
    let wide = app.agents_panes(Rect::new(0, 0, 160, 40), &selection);
    assert!(wide.work.is_none());
    assert!(app.selected_work(&selection).is_none());
}
