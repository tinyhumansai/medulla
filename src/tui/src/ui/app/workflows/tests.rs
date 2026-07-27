//! Tests for the Workflows tab's state: the rail cursor, the graph cache, and
//! the copilot thread.
//!
//! Every test drives a real store under a temporary `MEDULLA_HOME`, because the
//! rail and the canvas are both reading from disk and a stubbed store would not
//! exercise the reload paths that keep them agreeing with each other.

use std::sync::Arc;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use medulla::ui::workflows::Move;
use medulla::workflows::{WorkflowRecord, WorkflowStore};
use serde_json::json;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use super::super::types::{App, Cmd};

/// A workflow whose graph is trigger → check, branching to two agents that both
/// feed a merge: enough shape to test lanes, edges, and cursor movement.
fn diamond(id: &str) -> WorkflowRecord {
    WorkflowRecord {
        id: id.to_string(),
        name: format!("{id} workflow"),
        description: String::new(),
        enabled: true,
        graph: serde_json::from_value(json!({
            "name": id,
            "nodes": [
                { "id": "start", "kind": "trigger", "name": "Start",
                  "config": { "trigger_kind": "manual" } },
                { "id": "check", "kind": "condition", "name": "Check",
                  "config": { "expression": "=.ok" } },
                { "id": "yes", "kind": "agent", "name": "Yes",
                  "config": { "prompt": "go" } },
                { "id": "no", "kind": "agent", "name": "No", "config": { "prompt": "stop" } },
                { "id": "join", "kind": "merge", "name": "Join",
                  "config": { "inputs": ["a", "b"] } },
            ],
            "edges": [
                { "from_node": "start", "to_node": "check" },
                { "from_node": "check", "from_port": "true", "to_node": "yes" },
                { "from_node": "check", "from_port": "false", "to_node": "no" },
                { "from_node": "yes", "to_node": "join" },
                { "from_node": "no", "to_node": "join" },
            ],
        }))
        .expect("graph parses"),
        source_path: None,
    }
}

/// An app pointed at a temporary home holding `workflows`, already loaded.
fn app_with(workflows: &[WorkflowRecord]) -> (tempfile::TempDir, App) {
    let home = tempfile::tempdir().expect("tempdir");
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut app = App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()));
    app.set_medulla_home(home.path().to_path_buf());
    // Only the temp directory, so the catalogue is exactly what this test
    // installed rather than that plus whatever the checkout happens to hold.
    let store: Arc<dyn WorkflowStore> = Arc::new(medulla::workflows::FileWorkflowStore::new(
        vec![home.path().join("workflows")],
        home.path().join("runs"),
    ));
    app.set_workflow_store(store.clone());
    for workflow in workflows {
        store.save(workflow).expect("save");
    }
    app.reload_workflows();
    (home, app)
}

#[test]
fn entering_the_tab_reads_the_store_and_lays_the_first_graph_out() {
    let (_home, app) = app_with(&[diamond("sweep")]);

    assert_eq!(app.workflow_row_count(), 1);
    assert_eq!(app.workflow_layout().nodes.len(), 5);
    assert_eq!(app.workflow_layout().layers, 4);
}

#[test]
fn an_empty_store_leaves_the_canvas_empty_rather_than_stale() {
    let (_home, app) = app_with(&[]);

    assert_eq!(app.workflow_row_count(), 0);
    // The New row is still there: an empty machine is exactly when it matters.
    assert!(matches!(
        app.workflow_rail_rows().as_slice(),
        [super::WorkflowRailRow::New]
    ));
    assert!(app.workflow_layout().nodes.is_empty());
    assert!(app.selected_graph_node().is_none());
}

#[test]
fn the_rail_lists_the_selected_workflows_runs_and_no_others() {
    let (_home, app) = app_with(&[diamond("a"), diamond("b")]);

    let rows = app.workflow_rail_rows();

    // Two workflows, a note under the selected one saying it has no runs, and
    // the New row that closes every catalogue.
    assert_eq!(rows.len(), 4);
    assert!(matches!(
        rows[1],
        super::WorkflowRailRow::Note("no runs yet")
    ));
    assert!(matches!(rows[2], super::WorkflowRailRow::Workflow { .. }));
    assert!(matches!(rows[3], super::WorkflowRailRow::New));
}

#[test]
fn the_rail_cursor_walks_between_workflows() {
    let (_home, mut app) = app_with(&[diamond("a"), diamond("b")]);

    app.move_workflow_rail(false);

    assert_eq!(app.selected_workflow().unwrap().id, "b");
    let rows = app.workflow_rail_rows();
    // The runs now nest under `b`, so `b`'s own row is the second one.
    assert!(
        app.workflow_rail_selected(&rows[1]),
        "the cursor is on the second workflow's own row"
    );
    assert!(
        !app.workflow_rail_selected(&rows[0]),
        "and not still on the first"
    );
}

#[test]
fn the_rail_cursor_stops_at_both_ends_rather_than_wrapping() {
    let (_home, mut app) = app_with(&[diamond("a"), diamond("b")]);

    app.move_workflow_rail(true);
    assert_eq!(app.selected_workflow().unwrap().id, "a");

    app.move_workflow_rail(false);
    app.move_workflow_rail(false);
    assert_eq!(app.selected_workflow().unwrap().id, "b");
}

#[test]
fn selecting_a_workflow_reloads_the_graph_beside_it() {
    let mut second = diamond("b");
    second.graph.nodes.truncate(1);
    second.graph.edges.clear();
    let (_home, mut app) = app_with(&[diamond("a"), second]);

    app.move_workflow_rail(false);

    assert_eq!(
        app.workflow_layout().nodes.len(),
        1,
        "the canvas must never show the previous workflow's graph"
    );
}

#[test]
fn the_canvas_cursor_follows_edges_forward_and_back() {
    let (_home, mut app) = app_with(&[diamond("sweep")]);

    assert_eq!(app.selected_graph_node().unwrap().id, "start");
    app.move_graph_cursor(Move::Forward);
    assert_eq!(app.selected_graph_node().unwrap().id, "check");
    app.move_graph_cursor(Move::Forward);
    assert_eq!(app.selected_graph_node().unwrap().id, "yes");
    app.move_graph_cursor(Move::Back);
    assert_eq!(app.selected_graph_node().unwrap().id, "check");
}

#[test]
fn the_canvas_cursor_walks_the_lanes_of_a_branch() {
    let (_home, mut app) = app_with(&[diamond("sweep")]);
    app.move_graph_cursor(Move::Forward);
    app.move_graph_cursor(Move::Forward);

    app.move_graph_cursor(Move::LaneDown);

    assert_eq!(app.selected_graph_node().unwrap().id, "no");
}

#[test]
fn a_move_with_nowhere_to_go_leaves_the_cursor_where_it_was() {
    let (_home, mut app) = app_with(&[diamond("sweep")]);

    app.move_graph_cursor(Move::Back);

    assert_eq!(
        app.selected_graph_node().unwrap().id,
        "start",
        "the first step has nothing before it, and wrapping would read as a jump"
    );
}

#[test]
fn the_canvas_scrolls_to_keep_the_cursor_on_screen() {
    // A long chain, so the last node is well past one screen of layers.
    let mut chain = diamond("long");
    chain.graph = serde_json::from_value(json!({
        "nodes": (0..12).map(|index| json!({
            "id": format!("n{index}"),
            "kind": if index == 0 { "trigger" } else { "transform" },
            "name": format!("Step {index}"),
            "config": {},
        })).collect::<Vec<_>>(),
        "edges": (0..11).map(|index| json!({
            "from_node": format!("n{index}"), "to_node": format!("n{}", index + 1),
        })).collect::<Vec<_>>(),
    }))
    .expect("graph parses");
    let (_home, mut app) = app_with(&[chain]);

    for _ in 0..11 {
        app.move_graph_cursor(Move::Forward);
    }

    assert_eq!(app.selected_graph_node().unwrap().id, "n11");
    let node = app.selected_graph_node().unwrap().clone();
    assert!(
        node.layer >= app.wf.canvas_layer
            && node.layer < app.wf.canvas_layer + app.visible_layers(),
        "layer {} is outside the viewport at {}",
        node.layer,
        app.wf.canvas_layer
    );
}

#[test]
fn each_workflow_keeps_its_own_copilot_thread() {
    let (_home, mut app) = app_with(&[diamond("a"), diamond("b")]);

    app.copilot_mut().unwrap().ask("first");
    app.move_workflow_rail(false);

    assert!(
        app.copilot().is_none(),
        "the second workflow has not been asked anything"
    );
    app.move_workflow_rail(true);
    assert_eq!(app.copilot().unwrap().turns.len(), 1);
}

#[test]
fn submitting_an_instruction_records_it_and_asks_for_a_turn() {
    let (_home, mut app) = app_with(&[diamond("sweep")]);
    app.wf.draft = crate::ui::composer::insert_at("", 0, "add a slack step");

    let cmd = app.submit_copilot().expect("a turn is dispatched");

    assert!(matches!(
        cmd,
        super::super::types::Cmd::CopilotTurn { ref workflow, ref instruction }
            if workflow == "sweep" && instruction == "add a slack step"
    ));
    assert!(app.copilot_busy());
    assert!(app.wf.draft.text.is_empty(), "the composer is cleared");
}

#[test]
fn an_empty_instruction_dispatches_nothing() {
    let (_home, mut app) = app_with(&[diamond("sweep")]);
    app.wf.draft = crate::ui::composer::insert_at("", 0, "   ");

    assert!(app.submit_copilot().is_none());
    assert!(!app.copilot_busy());
}

#[test]
fn a_second_instruction_is_refused_while_a_turn_is_in_flight() {
    let (_home, mut app) = app_with(&[diamond("sweep")]);
    app.wf.draft = crate::ui::composer::insert_at("", 0, "one");
    app.submit_copilot().expect("first turn");
    app.wf.draft = crate::ui::composer::insert_at("", 0, "two");

    assert!(
        app.submit_copilot().is_none(),
        "the agent is editing the graph the second instruction was written against"
    );
    assert!(app.status().contains("still working"), "{}", app.status());
}

#[test]
fn a_finished_turn_ends_the_thread_and_reports_what_changed() {
    let (_home, mut app) = app_with(&[diamond("sweep")]);
    app.wf.draft = crate::ui::composer::insert_at("", 0, "go");
    app.submit_copilot().expect("turn");

    app.copilot_status("sweep", "reading the graph".into());
    app.copilot_finished(
        "sweep",
        "added it".into(),
        vec!["+ node notify".into()],
        None,
    );

    let thread = app.copilot().expect("thread");
    assert!(!thread.busy);
    let texts: Vec<&str> = thread.turns.iter().map(|turn| turn.text.as_str()).collect();
    assert_eq!(
        texts,
        vec!["go", "reading the graph", "+ node notify", "added it"]
    );
}

#[test]
fn a_turn_that_changed_the_graph_reloads_the_canvas_from_the_store() {
    let (_home, mut app) = app_with(&[diamond("sweep")]);
    // Stand in for the copilot's tool calls: the store is where a real turn's
    // edits land, and reloading from it is the behaviour under test.
    let store = app.workflow_store();
    let mut record = store.get("sweep").unwrap().unwrap();
    record.graph.nodes.pop();
    record.graph.edges.clear();
    store.save(&record).unwrap();

    app.copilot_finished(
        "sweep",
        "trimmed it".into(),
        vec!["− node join".into()],
        None,
    );

    assert_eq!(app.workflow_layout().nodes.len(), 4);
}

#[test]
fn a_turn_that_changed_nothing_does_not_disturb_the_canvas() {
    let (_home, mut app) = app_with(&[diamond("sweep")]);
    app.wf.node_index = 3;

    app.copilot_finished("sweep", "it branches on ok".into(), Vec::new(), None);

    assert_eq!(
        app.wf.node_index, 3,
        "an answered question must not move the operator's cursor"
    );
}

#[test]
fn a_failed_turn_ends_the_thread_and_says_so_on_the_status_line() {
    let (_home, mut app) = app_with(&[diamond("sweep")]);
    app.wf.draft = crate::ui::composer::insert_at("", 0, "go");
    app.submit_copilot().expect("turn");

    app.copilot_failed("sweep", "no harness installed".into());

    assert!(!app.copilot().unwrap().busy);
    assert!(
        app.status().contains("no harness installed"),
        "{}",
        app.status()
    );
}

#[test]
fn progress_for_a_workflow_with_no_thread_is_dropped_rather_than_creating_one() {
    let (_home, mut app) = app_with(&[diamond("sweep")]);

    app.copilot_status("someone-elses-workflow", "hello".into());

    assert!(app.copilot().is_none());
}

#[test]
fn the_new_row_closes_the_catalogue_and_the_cursor_walks_onto_it() {
    let (_home, mut app) = app_with(&[diamond("a")]);

    // Down past the last workflow's last row lands on New rather than stopping.
    app.move_workflow_rail(false);
    assert!(app.wf.creating, "down from the bottom reaches New");
    assert!(app
        .workflow_rail_rows()
        .iter()
        .any(|row| app.workflow_rail_selected(row) && matches!(row, super::WorkflowRailRow::New)));

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
