//! Click-through between the orchestrator's conversation and the sessions it
//! started, and the chord back.

use super::rail::tests::app;
use super::rail::RailRow;
use super::types::tab_pos;

#[test]
fn the_orchestrator_lists_the_sessions_it_started() {
    let app = app();
    let started = app.started_sessions();
    assert!(
        !started.is_empty(),
        "the demo fixture dispatches at least one task"
    );
    for session in &started {
        assert!(!session.task_id.is_empty(), "each entry names its task");
        assert!(
            matches!(
                app.rail_rows().get(session.row_index),
                Some(RailRow::Session(_))
            ),
            "each entry points at a session row"
        );
    }
}

#[test]
fn opening_an_entry_moves_the_rail_selection_onto_that_session() {
    let mut app = app();
    let entry = app
        .started_sessions()
        .into_iter()
        .next()
        .expect("a started session");

    assert!(app.focus_session_for_task(&entry.task_id));

    assert_eq!(app.tab(), "Agents");
    assert_eq!(app.agent_index(), entry.row_index, "the rail follows");
    assert_eq!(
        app.rail_rows()
            .get(app.agent_index())
            .and_then(RailRow::task)
            .map(|task| task.task_id.clone()),
        Some(entry.task_id),
        "the pane is now that session's conversation"
    );
    // No composer is drawn under a session row, so the keyboard must not be left
    // on one.
    assert!(app.agents_rail_focused());
    assert!(app.status().contains("^O"), "{}", app.status());
}

#[test]
fn a_task_no_session_is_serving_is_reported_by_name() {
    let mut app = app();
    assert!(!app.focus_session_for_task("no-such-task"));
    assert!(app.status().contains("no-such-task"), "{}", app.status());
    assert!(app.on_orchestrator_lane(), "the cursor did not move");
}

#[test]
fn a_session_the_operator_started_is_not_claimed_by_the_orchestrator() {
    // The block says what the *orchestrator* started. A user-originated session
    // has no task, so it is filtered out twice over — by origin and by task —
    // and every entry that survives can name a task.
    let app = app();
    assert!(app
        .started_sessions()
        .iter()
        .all(|session| !session.task_id.is_empty()));
}

#[test]
fn the_chord_back_returns_to_the_orchestrator_and_its_composer() {
    let mut app = app();
    let entry = app
        .started_sessions()
        .into_iter()
        .next()
        .expect("a started session");
    app.focus_session_for_task(&entry.task_id);

    app.focus_orchestrator();

    assert_eq!(app.tab_index, tab_pos("Agents"));
    assert!(app.on_orchestrator_lane(), "the cursor is on the lane");
    assert!(
        app.agents_composer_shown(),
        "and the keyboard is in the text box"
    );
}
