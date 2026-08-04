//! Tests for authoring a new workflow through the copilot.

use super::*;

#[test]
fn the_new_row_closes_the_catalogue_and_the_cursor_walks_onto_it() {
    let (_home, mut app) = app_with(&[diamond("a")]);

    // Down past the last workflow's last row lands on New rather than stopping.
    app.move_workflow_rail(false);
    assert!(app.wf.creating, "down from the bottom reaches New");
    assert!(app
        .workflow_rail_rows()
        .iter()
        .any(|row| app.workflow_rail_selected(row)
            && matches!(row, super::super::WorkflowRailRow::New)));

    // And Up comes back into the catalogue.
    app.move_workflow_rail(true);
    assert!(!app.wf.creating);
    assert_eq!(app.selected_workflow().unwrap().id, "a");
}

#[test]
fn an_empty_machine_still_offers_to_make_one() {
    let (_home, mut app) = app_with(&[]);

    app.move_workflow_rail(false);

    assert!(app.wf.creating, "the New row is the only thing to select");
    // And it stays there rather than walking off the end of an empty list.
    app.move_workflow_rail(false);
    assert!(app.wf.creating);
}

#[test]
fn the_new_row_has_a_thread_of_its_own() {
    let (_home, mut app) = app_with(&[diamond("a")]);
    // Something said to the existing workflow's copilot...
    app.wf.draft = crate::ui::composer::insert_at("", 0, "add a step");
    app.submit_copilot().expect("turn");

    app.wf.creating = true;

    // ...is not in the New row's conversation.
    assert!(
        app.copilot().is_none_or(|thread| thread.turns.is_empty()),
        "the New row starts empty"
    );
}

#[test]
fn describing_a_new_workflow_asks_for_a_create_rather_than_an_edit() {
    let (_home, mut app) = app_with(&[diamond("a")]);
    app.wf.creating = true;
    app.wf.draft = crate::ui::composer::insert_at("", 0, "summarise new issues daily");

    let cmd = app.submit_copilot().expect("a command");

    match cmd {
        Cmd::CreateWorkflow { instruction, .. } => {
            assert_eq!(instruction, "summarise new issues daily");
        }
        other => panic!("expected a create, got {other:?}"),
    }
    // The instruction is recorded in the New row's own thread, and it is busy.
    let thread = app.copilot().expect("the new thread");
    assert_eq!(thread.turns[0].text, "summarise new issues daily");
    assert!(thread.busy);
}

#[test]
fn a_created_workflow_is_selected_and_keeps_the_thread_that_made_it() {
    let (_home, mut app) = app_with(&[diamond("a")]);
    app.wf.creating = true;
    app.wf.draft = crate::ui::composer::insert_at("", 0, "build me one");
    let cmd = app.submit_copilot().expect("a command");
    let Cmd::CreateWorkflow { thread, .. } = cmd else {
        panic!("expected a create");
    };
    // The turn ran and the agent really did write a workflow to the store.
    app.workflow_store().save(&diamond("fresh")).expect("save");

    app.copilot_finished(
        &thread,
        "built it".into(),
        vec!["+ workflow fresh".into()],
        Some("fresh".into()),
    );

    assert!(!app.wf.creating, "the cursor leaves the New row");
    assert_eq!(app.selected_workflow().unwrap().id, "fresh");
    // The conversation moved with it: it is why this workflow looks like this.
    let thread = app.copilot().expect("the adopted thread");
    assert_eq!(thread.workflow_id, "fresh");
    assert!(thread.turns.iter().any(|turn| turn.text == "build me one"));
}

#[test]
fn a_follow_up_queued_during_a_create_turn_drains_under_the_adopted_id() {
    // Regression: `adopt_new_workflow` moves the thread — queue and all — from
    // `NEW_THREAD` to the created workflow's real id. Draining under the
    // original (pre-move) key would look up a thread that no longer lives
    // there and silently drop the operator's queued follow-up.
    let (_home, mut app) = app_with(&[diamond("a")]);
    app.wf.creating = true;
    app.wf.draft = crate::ui::composer::insert_at("", 0, "build me one");
    let cmd = app.submit_copilot().expect("a command");
    let Cmd::CreateWorkflow { thread, .. } = cmd else {
        panic!("expected a create");
    };

    // Queued while the create turn is still in flight.
    app.wf.draft = crate::ui::composer::insert_at("", 0, "now add a step");
    assert!(
        app.submit_copilot().is_none(),
        "busy — this must queue, not dispatch"
    );

    app.workflow_store().save(&diamond("fresh")).expect("save");
    let drained = app.copilot_finished(
        &thread,
        "built it".into(),
        vec!["+ workflow fresh".into()],
        Some("fresh".into()),
    );

    let Some(Cmd::CopilotTurn {
        workflow,
        instruction,
    }) = drained
    else {
        panic!("the queued follow-up must be drained, not dropped");
    };
    assert_eq!(workflow, "fresh", "drained under the adopted id");
    assert_eq!(instruction, "now add a step");
}

#[test]
fn a_create_turn_that_built_nothing_leaves_the_cursor_where_it_was() {
    let (_home, mut app) = app_with(&[diamond("a")]);
    app.wf.creating = true;
    app.wf.draft = crate::ui::composer::insert_at("", 0, "what could I build?");
    let cmd = app.submit_copilot().expect("a command");
    let Cmd::CreateWorkflow { thread, .. } = cmd else {
        panic!("expected a create");
    };

    // A question, answered. Nothing was created and nothing changed.
    app.copilot_finished(&thread, "you could try…".into(), Vec::new(), None);

    assert!(app.wf.creating, "still on the New row");
    let turns = &app.copilot().expect("the new thread").turns;
    assert_eq!(turns.last().unwrap().text, "you could try…");
    assert!(!app.copilot_busy());
}

#[test]
fn there_is_nothing_to_run_from_the_new_row() {
    let (_home, mut app) = app_with(&[diamond("a")]);
    app.wf.creating = true;

    // `x` and `d` must not act on whichever workflow the index happens to hold.
    for key in [KeyCode::Char('x'), KeyCode::Char('d')] {
        let before = app.wf.creating;
        app.on_event(Event::Key(KeyEvent::new(key, KeyModifiers::NONE)));
        assert_eq!(app.wf.creating, before);
    }
}
