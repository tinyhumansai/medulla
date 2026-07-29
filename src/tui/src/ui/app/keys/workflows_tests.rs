//! Tests for the Workflows tab's key handling: which pane consumes a key, and
//! what focus/cursor/draft change it makes.
//!
//! Driven directly through [`App::on_workflows_key`] rather than the full
//! [`App::on_key`] dispatcher, so a test states exactly which pane is focused
//! rather than steering there through a chain of presses.

use std::sync::Arc;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use medulla::workflows::{WorkflowRecord, WorkflowStore};
use serde_json::json;

use super::*;

/// A single-node workflow: enough to select and act on, with no room to move
/// the graph cursor forward or sideways.
fn solo(id: &str) -> WorkflowRecord {
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
            ],
            "edges": [],
        }))
        .expect("graph parses"),
        source_path: None,
    }
}

/// A workflow whose graph branches, so the canvas cursor has somewhere to go
/// forward, back, and sideways between lanes.
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

/// A disabled workflow: `d`/`x` must refuse it either from the sidebar or the
/// canvas, without dispatching a command.
fn disabled(id: &str) -> WorkflowRecord {
    let mut record = solo(id);
    record.enabled = false;
    record
}

/// An app pointed at a temporary home holding `workflows`, already loaded.
fn app_with(workflows: &[WorkflowRecord]) -> (tempfile::TempDir, App) {
    let home = tempfile::tempdir().expect("tempdir");
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut app = App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()));
    app.set_medulla_home(home.path().to_path_buf());
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

fn key(app: &mut App, code: KeyCode) -> WorkflowsKey {
    app.on_workflows_key(code, false, false)
}

// ---- sidebar ----

#[test]
fn a_digit_jumps_to_the_workflow_at_that_position_and_opens_the_canvas() {
    let (_home, mut app) = app_with(&[solo("a-first"), solo("b-second")]);

    key(&mut app, KeyCode::Char('2'));

    assert_eq!(app.selected_workflow().unwrap().id, "b-second");
    assert_eq!(app.wf_focus(), WorkflowFocus::Canvas);
}

#[test]
fn a_digit_past_the_end_of_the_catalogue_does_nothing() {
    let (_home, mut app) = app_with(&[solo("only")]);

    let result = key(&mut app, KeyCode::Char('9'));

    assert!(matches!(result, WorkflowsKey::Handled(None)));
    assert_eq!(app.selected_workflow().unwrap().id, "only");
    assert_eq!(
        app.wf_focus(),
        WorkflowFocus::Sidebar,
        "an out-of-range digit must not step into the canvas"
    );
}

#[test]
fn enter_right_and_l_all_step_into_the_canvas_from_the_sidebar() {
    for code in [KeyCode::Enter, KeyCode::Right, KeyCode::Char('l')] {
        let (_home, mut app) = app_with(&[solo("sweep")]);

        key(&mut app, code);

        assert_eq!(app.wf_focus(), WorkflowFocus::Canvas, "{code:?}");
        assert!(app.status().contains("Graph"), "{}", app.status());
    }
}

#[test]
fn c_opens_the_copilot_from_the_sidebar() {
    let (_home, mut app) = app_with(&[solo("sweep")]);

    key(&mut app, KeyCode::Char('c'));

    assert_eq!(app.wf_focus(), WorkflowFocus::Copilot);
}

#[test]
fn d_simulates_and_x_runs_the_selected_workflow_from_the_sidebar() {
    let (_home, mut app) = app_with(&[solo("sweep")]);

    let dry = key(&mut app, KeyCode::Char('d'));
    assert!(matches!(
        dry,
        WorkflowsKey::Handled(Some(Cmd::DryRunWorkflow { ref id })) if id == "sweep"
    ));
    assert!(app.status().contains("Simulating"), "{}", app.status());

    let run = key(&mut app, KeyCode::Char('x'));
    assert!(matches!(
        run,
        WorkflowsKey::Handled(Some(Cmd::RunWorkflow { ref id })) if id == "sweep"
    ));
    assert!(app.status().contains("Running"), "{}", app.status());
}

#[test]
fn u_undoes_the_selected_workflow_from_either_pane() {
    let (_home, mut app) = app_with(&[solo("sweep")]);

    let from_sidebar = key(&mut app, KeyCode::Char('u'));
    assert!(matches!(
        from_sidebar,
        WorkflowsKey::Handled(Some(Cmd::UndoWorkflow { ref id })) if id == "sweep"
    ));

    // Also on the canvas: undo answers an edit the operator is *looking at*,
    // and stepping back out to the list to reach it would be a step away from
    // the thing they want changed.
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.wf_focus(), WorkflowFocus::Canvas);
    let from_canvas = key(&mut app, KeyCode::Char('u'));
    assert!(matches!(
        from_canvas,
        WorkflowsKey::Handled(Some(Cmd::UndoWorkflow { ref id })) if id == "sweep"
    ));
}

#[test]
fn u_on_the_new_row_says_there_is_nothing_to_undo_yet() {
    let (_home, mut app) = app_with(&[solo("sweep")]);
    // The New row sits below the whole catalogue, so Down from the last row of
    // the only workflow lands on it.
    key(&mut app, KeyCode::Down);

    let pressed = key(&mut app, KeyCode::Char('u'));

    assert!(matches!(pressed, WorkflowsKey::Handled(None)));
    assert!(
        app.status().contains("not been created"),
        "{}",
        app.status()
    );
}

#[test]
fn x_refuses_a_disabled_workflow_from_the_sidebar_but_d_still_simulates_it() {
    let (_home, mut app) = app_with(&[disabled("paused")]);

    // A dry run starts no harness session, so it stays available on a
    // disabled workflow — the safe half of the question `x` refuses.
    let dry = key(&mut app, KeyCode::Char('d'));
    assert!(matches!(
        dry,
        WorkflowsKey::Handled(Some(Cmd::DryRunWorkflow { ref id })) if id == "paused"
    ));

    let run = key(&mut app, KeyCode::Char('x'));
    assert!(matches!(run, WorkflowsKey::Handled(None)));
    assert!(app.status().contains("disabled"), "{}", app.status());
}

#[test]
fn a_stray_letter_is_swallowed_rather_than_falling_through_to_a_global_binding() {
    let (_home, mut app) = app_with(&[solo("sweep")]);

    let result = key(&mut app, KeyCode::Char('z'));

    assert!(matches!(result, WorkflowsKey::Handled(None)));
    assert_eq!(app.wf_focus(), WorkflowFocus::Sidebar);
}

#[test]
fn a_non_character_key_the_sidebar_does_not_bind_is_unhandled() {
    let (_home, mut app) = app_with(&[solo("sweep")]);

    let result = key(&mut app, KeyCode::Tab);

    assert!(matches!(result, WorkflowsKey::Unhandled));
}

// ---- canvas ----

#[test]
fn esc_on_the_canvas_always_steps_back_to_the_list() {
    let (_home, mut app) = app_with(&[diamond("sweep")]);
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Right); // off the first node, so Left alone would not exit

    key(&mut app, KeyCode::Esc);

    assert_eq!(app.wf_focus(), WorkflowFocus::Sidebar);
    assert!(app.status().contains("list"), "{}", app.status());
}

#[test]
fn left_off_the_first_node_steps_back_to_the_list() {
    let (_home, mut app) = app_with(&[diamond("sweep")]);
    key(&mut app, KeyCode::Enter);

    key(&mut app, KeyCode::Left);

    assert_eq!(app.wf_focus(), WorkflowFocus::Sidebar);
}

#[test]
fn left_with_room_moves_the_cursor_back_instead_of_leaving_the_canvas() {
    let (_home, mut app) = app_with(&[diamond("sweep")]);
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Right); // start -> check

    key(&mut app, KeyCode::Left); // check -> start

    assert_eq!(app.wf_focus(), WorkflowFocus::Canvas);
    assert_eq!(app.selected_graph_node().unwrap().id, "start");
}

#[test]
fn right_and_h_and_k_and_j_move_the_graph_cursor() {
    let (_home, mut app) = app_with(&[diamond("sweep")]);
    key(&mut app, KeyCode::Enter);

    key(&mut app, KeyCode::Right);
    assert_eq!(app.selected_graph_node().unwrap().id, "check");

    key(&mut app, KeyCode::Char('l'));
    assert_eq!(app.selected_graph_node().unwrap().id, "yes");

    key(&mut app, KeyCode::Char('h'));
    assert_eq!(app.selected_graph_node().unwrap().id, "check");

    key(&mut app, KeyCode::Char('l'));
    key(&mut app, KeyCode::Char('j'));
    assert_eq!(app.selected_graph_node().unwrap().id, "no");

    key(&mut app, KeyCode::Char('k'));
    assert_eq!(app.selected_graph_node().unwrap().id, "yes");
}

#[test]
fn i_toggles_the_inspector() {
    let (_home, mut app) = app_with(&[diamond("sweep")]);
    key(&mut app, KeyCode::Enter);
    assert!(!app.wf_inspector_open());

    key(&mut app, KeyCode::Char('i'));
    assert!(app.wf_inspector_open());

    key(&mut app, KeyCode::Char('i'));
    assert!(!app.wf_inspector_open());
}

#[test]
fn c_opens_the_copilot_from_the_canvas() {
    let (_home, mut app) = app_with(&[diamond("sweep")]);
    key(&mut app, KeyCode::Enter);

    key(&mut app, KeyCode::Char('c'));

    assert_eq!(app.wf_focus(), WorkflowFocus::Copilot);
}

#[test]
fn r_reloads_the_store_from_the_canvas() {
    let (_home, mut app) = app_with(&[diamond("sweep")]);
    key(&mut app, KeyCode::Enter);

    key(&mut app, KeyCode::Char('r'));

    assert!(app.status().contains("workflow"), "{}", app.status());
}

#[test]
fn d_simulates_and_x_runs_from_the_canvas() {
    let (_home, mut app) = app_with(&[diamond("sweep")]);
    key(&mut app, KeyCode::Enter);

    let dry = key(&mut app, KeyCode::Char('d'));
    assert!(matches!(
        dry,
        WorkflowsKey::Handled(Some(Cmd::DryRunWorkflow { ref id })) if id == "sweep"
    ));

    let run = key(&mut app, KeyCode::Char('x'));
    assert!(matches!(
        run,
        WorkflowsKey::Handled(Some(Cmd::RunWorkflow { ref id })) if id == "sweep"
    ));
}

#[test]
fn a_key_the_canvas_does_not_bind_is_unhandled() {
    let (_home, mut app) = app_with(&[diamond("sweep")]);
    key(&mut app, KeyCode::Enter);

    let result = key(&mut app, KeyCode::Tab);

    assert!(matches!(result, WorkflowsKey::Unhandled));
}

// ---- copilot ----

/// An app parked in the copilot, its draft already holding `text`.
fn app_in_copilot(text: &str) -> (tempfile::TempDir, App) {
    let (home, mut app) = app_with(&[diamond("sweep")]);
    key(&mut app, KeyCode::Char('c'));
    for ch in text.chars() {
        app.on_workflows_key(KeyCode::Char(ch), false, false);
    }
    (home, app)
}

#[test]
fn typed_characters_land_in_the_draft_in_order() {
    let (_home, app) = app_in_copilot("hi");

    assert_eq!(app.wf.draft.text, "hi");
}

#[test]
fn shift_or_alt_enter_inserts_a_newline_instead_of_submitting() {
    let (_home, mut app) = app_in_copilot("go");

    let shift = app.on_workflows_key(KeyCode::Enter, true, false);
    assert!(matches!(shift, WorkflowsKey::Handled(None)));
    assert_eq!(app.wf.draft.text, "go\n");

    let alt = app.on_workflows_key(KeyCode::Enter, false, true);
    assert!(matches!(alt, WorkflowsKey::Handled(None)));
    assert_eq!(app.wf.draft.text, "go\n\n");
}

#[test]
fn plain_enter_submits_the_draft_as_a_copilot_turn() {
    let (_home, mut app) = app_in_copilot("add a slack step");

    let result = key(&mut app, KeyCode::Enter);

    assert!(matches!(
        result,
        WorkflowsKey::Handled(Some(Cmd::CopilotTurn { ref workflow, ref instruction }))
            if workflow == "sweep" && instruction == "add a slack step"
    ));
    assert!(app.wf.draft.text.is_empty());
}

#[test]
fn backspace_and_delete_remove_the_character_before_the_cursor() {
    let (_home, mut app) = app_in_copilot("hit");

    key(&mut app, KeyCode::Backspace);
    assert_eq!(app.wf.draft.text, "hi");

    key(&mut app, KeyCode::Delete);
    assert_eq!(app.wf.draft.text, "h");
}

#[test]
fn left_and_right_move_the_cursor_without_editing_the_draft() {
    let (_home, mut app) = app_in_copilot("ab");
    assert_eq!(app.wf.draft.cursor, 2);

    key(&mut app, KeyCode::Left);
    assert_eq!(app.wf.draft.cursor, 1);

    // Typing at the moved cursor proves it actually moved rather than the
    // draft only reporting a stale count.
    key(&mut app, KeyCode::Char('x'));
    assert_eq!(app.wf.draft.text, "axb");

    // Right stops at the end rather than running past it.
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Right);
    key(&mut app, KeyCode::Right);
    assert_eq!(app.wf.draft.cursor, 3);
}

#[test]
fn page_up_and_page_down_scroll_the_transcript_and_stop_at_zero() {
    let (_home, mut app) = app_in_copilot("");

    // A page, sized from the terminal, rather than a fixed five rows — so the
    // step matches what the pane actually shows. The render clamps it to what
    // the content can scroll by.
    let page = app.copilot_page();
    assert!(page > 0, "a page must move at least one row");

    key(&mut app, KeyCode::PageUp);
    assert_eq!(app.wf.copilot_scroll, page);

    key(&mut app, KeyCode::PageDown);
    key(&mut app, KeyCode::PageDown);
    assert_eq!(
        app.wf.copilot_scroll, 0,
        "scrolling past the bottom must not wrap negative"
    );
}

#[test]
fn esc_clears_a_draft_before_it_leaves_the_composer() {
    let (_home, mut app) = app_in_copilot("half-typed");

    key(&mut app, KeyCode::Esc);
    assert!(app.wf.draft.text.is_empty());
    assert_eq!(
        app.wf_focus(),
        WorkflowFocus::Copilot,
        "the first Esc clears the draft, not the pane"
    );

    key(&mut app, KeyCode::Esc);
    assert_eq!(app.wf_focus(), WorkflowFocus::Canvas);
}

#[test]
fn alt_held_characters_are_not_typed_into_the_draft() {
    let (_home, mut app) = app_with(&[diamond("sweep")]);
    key(&mut app, KeyCode::Char('c'));

    let result = app.on_workflows_key(KeyCode::Char('x'), false, true);

    assert!(matches!(result, WorkflowsKey::Unhandled));
    assert!(app.wf.draft.text.is_empty());
}

#[test]
fn r_in_the_copilot_retries_only_when_nothing_is_typed() {
    let (_home, mut app) = app_with(&[solo("sweep")]);
    key(&mut app, KeyCode::Char('c'));
    assert_eq!(app.wf_focus(), WorkflowFocus::Copilot);

    // With a draft in progress `r` is a letter, because every printable key in
    // this pane types — an `r` that sometimes did something else would be
    // worse than no retry at all.
    key(&mut app, KeyCode::Char('g'));
    key(&mut app, KeyCode::Char('r'));
    assert_eq!(app.wf.draft.text, "gr");

    // Emptied, so `r` is the retry again.
    for _ in 0..2 {
        key(&mut app, KeyCode::Backspace);
    }
    let pressed = key(&mut app, KeyCode::Char('r'));
    assert!(matches!(pressed, WorkflowsKey::Handled(None)));
    assert!(
        app.status().contains("Nothing to retry"),
        "{}",
        app.status()
    );
}

#[test]
fn f_with_no_run_selected_says_so_rather_than_falling_through_silently() {
    let (_home, mut app) = app_with(&[solo("sweep")]);

    // No runs at all, so the cursor is on the workflow rather than a run.
    let pressed = key(&mut app, KeyCode::Char('f'));

    // `Handled(None)` alone would also be what a stray letter absorbed by the
    // sidebar's generic handler looks like; the status is what proves `f` was
    // actually recognised and refused for a stated reason.
    assert!(matches!(pressed, WorkflowsKey::Handled(None)));
    assert!(app.status().contains("No run selected"), "{}", app.status());
}

#[test]
fn f_on_a_run_that_did_not_fail_says_there_is_nothing_to_repair() {
    let (_home, mut app) = app_with(&[solo("sweep")]);
    let mut run = medulla::workflows::new_run_record("run-1", "sweep", 0);
    run.status = medulla::workflows::RunStatus::Succeeded;
    app.workflow_store().record_run(&run).expect("record");
    app.reload_workflows();

    // Down from the workflow row lands on its first (and only) run.
    key(&mut app, KeyCode::Down);
    let pressed = key(&mut app, KeyCode::Char('f'));

    assert!(matches!(pressed, WorkflowsKey::Handled(None)));
    assert!(app.status().contains("did not fail"), "{}", app.status());
}
