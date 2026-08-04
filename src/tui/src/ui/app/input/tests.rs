//! Tests for the navigation helpers the keyboard and the pointer share —
//! currently the paging of a lane's task sublanes behind its `+N more` row.

use std::sync::Arc;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use medulla::ui::agents::AgentRow;
use medulla::ui::events::{EventEnvelope, TuiEvent};

use super::super::rail::RailRow;
use super::super::types::App;
use super::nav::SUBTASK_PAGE;

/// An app whose event stream is only the tasks these tests are about, so the
/// demo runtime's own lanes cannot shift the rows out from under an assertion.
fn app_with_tasks(agent: &str, tasks: usize) -> App {
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut app = App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()));
    app.snapshot.events.clear();
    push_tasks(&mut app, agent, tasks);
    app
}

/// Append `tasks` started tasks for `agent`, continuing the event sequence.
fn push_tasks(app: &mut App, agent: &str, tasks: usize) {
    let base = app.snapshot.events.len() as u64;
    for i in 0..tasks {
        let seq = base + i as u64;
        app.snapshot.events.push(EventEnvelope {
            seq,
            at: seq as i64 * 1000,
            event: TuiEvent::TaskStart {
                task_id: format!("{agent}-t{i}"),
                instruction: "x".into(),
                depth: 2,
                agent_id: Some(agent.to_string()),
                contract: None,
            },
        });
    }
}

/// How many task sublanes the rail is currently drawing.
fn shown_subtasks(app: &App) -> usize {
    app.agent_rows()
        .iter()
        .filter(|row| matches!(row, AgentRow::Sub { .. }))
        .count()
}

/// Put the cursor on the overflow row, as arrowing onto it would.
fn select_overflow_row(app: &mut App) {
    let index = app
        .rail_rows()
        .iter()
        .position(|row| matches!(row, RailRow::Agent(AgentRow::More { .. })))
        .expect("a lane with hidden sublanes has an overflow row");
    app.agent_index = index;
}

/// Whether the cursor is on an overflow row right now.
fn on_overflow_row(app: &App) -> bool {
    matches!(
        app.rail_rows().get(app.agent_index),
        Some(RailRow::Agent(AgentRow::More { .. }))
    )
}

#[test]
fn the_overflow_row_reveals_one_more_page_each_time() {
    let mut app = app_with_tasks("dev", 25);
    assert_eq!(shown_subtasks(&app), SUBTASK_PAGE);

    select_overflow_row(&mut app);
    assert!(app.page_subtasks());
    assert_eq!(shown_subtasks(&app), SUBTASK_PAGE * 2);

    // The cursor rode down with the row it acted on, so the next press pages
    // again without the operator having to chase it.
    assert!(on_overflow_row(&app));
    assert!(app.page_subtasks());
    assert_eq!(shown_subtasks(&app), 25);
}

#[test]
fn a_fully_revealed_lane_folds_back_to_one_page() {
    let mut app = app_with_tasks("dev", 25);
    select_overflow_row(&mut app);
    app.page_subtasks();
    app.page_subtasks();
    assert_eq!(shown_subtasks(&app), 25);

    // Nothing is hidden now, so the same row is the way back.
    assert!(on_overflow_row(&app));
    assert!(app.page_subtasks());
    assert_eq!(shown_subtasks(&app), SUBTASK_PAGE);
}

#[test]
fn paging_elsewhere_leaves_other_rows_alone() {
    let mut app = app_with_tasks("dev", 25);
    // Row 0 is a lane header, not an overflow row.
    app.agent_index = 0;
    assert!(!app.page_subtasks());
    assert_eq!(shown_subtasks(&app), SUBTASK_PAGE);
    // The cursor is left where it was, so Enter can fall through to its
    // ordinary meaning.
    assert_eq!(app.agent_index, 0);
}

#[test]
fn an_expansion_follows_its_lane_when_the_rows_move() {
    let mut app = app_with_tasks("dev", 25);
    select_overflow_row(&mut app);
    app.page_subtasks();
    assert_eq!(shown_subtasks(&app), SUBTASK_PAGE * 2);

    // A second agent starts work. Its lane sorts among the existing ones and
    // shifts the rail indexes; the expansion is keyed to the lane, so it stays
    // with the agent it was opened on rather than jumping to a neighbour.
    push_tasks(&mut app, "ops", 3);
    let expanded = app
        .agent_rows()
        .iter()
        .filter(|row| matches!(row, AgentRow::Sub { .. }))
        .count();
    assert_eq!(expanded, SUBTASK_PAGE * 2 + 3);
}
