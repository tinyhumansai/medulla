//! Rendering tests for the Workflows tab.
//!
//! Every test draws the whole tab onto a [`TestBackend`] and reads the resulting
//! screen, because the thing under test is what an operator sees: that the three
//! panes are all present, that the graph is drawn as marked nodes joined by
//! wires, that its wires carry a moving flow highlight, and that a run overlaid
//! on it marks the steps it reached.

use std::sync::Arc;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use medulla::ui::workflows::Move;
use medulla::workflows::{RunRecord, RunStatus, RunStep, WorkflowRecord, WorkflowStore};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use serde_json::json;

use super::super::super::types::{App, WorkflowFocus, TABS};

/// A workflow branching two ways and merging again.
fn diamond(id: &str) -> WorkflowRecord {
    WorkflowRecord {
        id: id.to_string(),
        name: format!("{id} sweep"),
        description: String::new(),
        enabled: true,
        defaults: Default::default(),
        graph: serde_json::from_value(json!({
            "name": id,
            "nodes": [
                { "id": "start", "kind": "trigger", "name": "Start",
                  "config": { "trigger_kind": "manual" } },
                { "id": "check", "kind": "condition", "name": "Check",
                  "config": { "expression": "=.ok" } },
                { "id": "yes", "kind": "agent", "name": "Yes", "config": { "prompt": "go" } },
                { "id": "no", "kind": "agent", "name": "No", "config": { "prompt": "stop" } },
                { "id": "join", "kind": "merge", "name": "Join", "config": { "inputs": ["a"] } },
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

/// A workflow with enough parallel lanes to require vertical graph scrolling.
fn fanout(id: &str) -> WorkflowRecord {
    let nodes = std::iter::once(json!({
        "id": "start", "kind": "trigger", "name": "Start",
        "config": { "trigger_kind": "manual" }
    }))
    .chain((0..8).map(|index| {
        json!({
            "id": format!("agent-{index}"),
            "kind": "agent",
            "name": format!("Agent {index}"),
            "config": { "prompt": "go" }
        })
    }))
    .collect::<Vec<_>>();
    let edges = (0..8)
        .map(|index| json!({ "from_node": "start", "to_node": format!("agent-{index}") }))
        .collect::<Vec<_>>();
    WorkflowRecord {
        id: id.to_string(),
        name: format!("{id} fanout"),
        description: String::new(),
        enabled: true,
        defaults: Default::default(),
        graph: serde_json::from_value(json!({
            "name": id,
            "nodes": nodes,
            "edges": edges,
        }))
        .expect("graph parses"),
        source_path: None,
    }
}

/// An app on the Workflows tab, with `workflows` installed and `runs` recorded.
fn app_with(workflows: &[WorkflowRecord], runs: &[RunRecord]) -> (tempfile::TempDir, App) {
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
    for run in runs {
        store.record_run(run).expect("record");
    }
    app.tab_index = TABS
        .iter()
        .position(|tab| *tab == "Workflows")
        .expect("the tab exists");
    app.reload_workflows();
    (home, app)
}

/// The whole screen, as one string.
fn render(app: &mut App) -> String {
    render_sized(app, 160, 40)
}

/// The whole screen at a given size, as one string.
fn render_sized(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|f| app.draw(f)).expect("draw");
    let buffer = terminal.backend().buffer().clone();
    // Row-wise, so a test can look for a whole line rather than a wrap artefact.
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn workflows_is_a_top_level_tab() {
    assert!(TABS.contains(&"Workflows"));
    assert!(
        !medulla_tui_routing_subpages().contains(&"Workflows"),
        "it moved out of Routing rather than being listed in both"
    );
}

/// The Routing subpages, for the test above.
fn medulla_tui_routing_subpages() -> &'static [&'static str] {
    &super::super::super::types::ROUTING_SUBPAGES
}

#[test]
fn the_tab_is_a_sidebar_beside_one_content_pane() {
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);

    let screen = render(&mut app);

    assert!(screen.contains("Workflows · 1"), "the rail is missing");
    assert!(
        screen.contains("nightly sweep"),
        "the canvas title is missing"
    );
    // One view at a time: the copilot is a keystroke away, not a third column
    // competing with the graph for the width.
    assert!(!screen.contains("Copilot"), "{screen}");
}

#[test]
fn canvas_navigation_uses_the_split_graph_panes_measured_height() {
    let (_home, mut app) = app_with(&[fanout("parallel")], &[]);

    render_sized(&mut app, 140, 34);
    let measured_rows = app.visible_rows();
    assert_eq!(measured_rows, app.wf.graph_rows.max(1));

    app.move_graph_cursor(Move::Forward);
    for _ in 0..7 {
        app.move_graph_cursor(Move::LaneDown);
    }
    let node = app.selected_graph_node().expect("selected node").clone();
    let (_, row) = app.graph_cell(node.layer, node.lane);
    assert!(
        row >= app.wf.canvas_row && row < app.wf.canvas_row + measured_rows,
        "row {row} must remain inside {}..{}",
        app.wf.canvas_row,
        app.wf.canvas_row + measured_rows
    );
    assert!(app.wf.canvas_row > 0, "the reduced pane must scroll");
}

#[test]
fn each_view_takes_the_content_pane_in_turn() {
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);

    // The graph is what the tab falls back to.
    assert!(render(&mut app).contains("▶ Start"));

    app.wf.inspector_open = true;
    let inspector = render(&mut app);
    assert!(inspector.contains("trigger_kind"), "{inspector}");
    assert!(!inspector.contains("▶ Start"), "one view at a time");

    // The copilot wins over the inspector: it is the one you step into.
    app.wf.focus = WorkflowFocus::Copilot;
    let copilot = render(&mut app);
    assert!(copilot.contains("Copilot"), "{copilot}");
    assert!(!copilot.contains("trigger_kind"), "one view at a time");

    // The sidebar is present throughout — it is how you get back.
    assert!(copilot.contains("Workflows · 1"), "{copilot}");
}

#[test]
fn long_workflow_names_cannot_widen_the_rail_past_the_agents_cap() {
    let mut workflow = diamond("nightly");
    workflow.name = "an extremely long workflow name ".repeat(8);
    let (_home, app) = app_with(&[workflow], &[]);

    assert_eq!(app.workflow_sidebar_width(200), 38);
}

#[test]
fn the_graph_is_drawn_as_marked_nodes_joined_by_wires() {
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);

    let screen = render(&mut app);

    assert!(screen.contains("▶ Start"), "the trigger node is missing");
    assert!(screen.contains("◆ Check"), "the condition node is missing");
    assert!(
        screen.contains("Start · trigger") && screen.contains("wheel/Page scroll"),
        "the selected step's persistent detail pane is missing: {screen}"
    );
    // One row per node, wires attaching on that same row: a node and the node
    // it feeds share a line. This is what buys the canvas the four-fold density
    // that drawing each node as a box cost.
    let row = screen
        .lines()
        .find(|line| line.contains("▶ Start"))
        .expect("the trigger is on some row");
    assert!(
        row.contains("◆ Check"),
        "a node sits inline with the node it feeds: {row}"
    );
    assert!(row.contains('─'), "joined by a wire on that row: {row}");
    assert!(screen.contains('▶'), "edges end in an arrowhead");
    assert!(
        screen.contains('│'),
        "a branch routes vertically between lanes"
    );
}

#[test]
fn a_branchs_port_names_are_drawn_on_the_wires_that_carry_them() {
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);

    let screen = render(&mut app);

    assert!(
        screen.contains("true") && screen.contains("false"),
        "which arm an edge is cannot be read off the picture otherwise: {screen}"
    );
}

#[test]
fn an_empty_store_says_so_and_points_at_the_copilot() {
    let (_home, mut app) = app_with(&[], &[]);

    let screen = render(&mut app);

    assert!(screen.contains("No workflows installed."));
    assert!(screen.contains("copilot"), "{screen}");
}

#[test]
fn the_selected_workflows_runs_are_listed_under_it_in_the_rail() {
    let run = RunRecord {
        id: "run-abc".into(),
        workflow_id: "nightly".into(),
        status: RunStatus::Succeeded,
        started_at: 1,
        finished_at: Some(2),
        steps: vec![RunStep {
            node_id: "start".into(),
            status: "success".into(),
            duration_ms: 3,
            input: None,
            output: None,
            diagnostics: Vec::new(),
            transcript: Vec::new(),
        }],
        pending_approvals: Vec::new(),
        error: None,
        inputs: Default::default(),
        trigger: None,
        origin: None,
        summary: None,
        diagnosis: None,
    };
    let (_home, mut app) = app_with(&[diamond("nightly")], &[run]);

    let screen = render(&mut app);

    assert!(screen.contains("succeeded"), "{screen}");
}

#[test]
fn selecting_a_run_overlays_it_on_the_graph() {
    let run = RunRecord {
        id: "run-abc".into(),
        workflow_id: "nightly".into(),
        status: RunStatus::Failed,
        started_at: 1,
        finished_at: Some(2),
        steps: vec![
            RunStep {
                node_id: "start".into(),
                status: "success".into(),
                duration_ms: 3,
                input: None,
                output: Some(json!({ "started": true })),
                diagnostics: Vec::new(),
                transcript: Vec::new(),
            },
            RunStep {
                node_id: "check".into(),
                status: "failed".into(),
                duration_ms: 4,
                input: None,
                output: None,
                diagnostics: Vec::new(),
                transcript: Vec::new(),
            },
        ],
        pending_approvals: Vec::new(),
        error: Some("boom".into()),
        inputs: Default::default(),
        trigger: None,
        origin: None,
        summary: None,
        diagnosis: None,
    };
    let (_home, mut app) = app_with(&[diamond("nightly")], &[run]);

    app.move_workflow_rail(false);
    let screen = render(&mut app);

    assert!(screen.contains("run "), "the title names the overlaid run");
    assert!(screen.contains('✓'), "a step that passed is marked");
    assert!(screen.contains('✗'), "the step that failed is marked");
    assert!(
        screen.contains("\"started\": true"),
        "the selected step's recorded result is shown: {screen}"
    );
    assert!(
        screen.contains("[f] Fix this run via agent"),
        "a failed run offers the existing repair action: {screen}"
    );
}

#[test]
fn the_inspector_is_a_view_of_its_own_that_i_opens_and_closes() {
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);

    // Closed, it is not on screen at all — the graph has the whole pane.
    let closed = render(&mut app);
    assert!(!closed.contains("trigger_kind"), "{closed}");

    app.wf.inspector_open = true;
    let open = render(&mut app);
    assert!(
        open.contains("trigger_kind"),
        "the open inspector shows the node's declaration: {open}"
    );
    assert!(open.contains("i back to the graph"), "{open}");
}

#[test]
fn the_inspector_follows_the_canvas_cursor() {
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);
    app.wf.inspector_open = true;

    app.move_graph_cursor(Move::Forward);
    let screen = render(&mut app);

    assert!(screen.contains("Check · condition"), "{screen}");
    assert!(screen.contains("=.ok"));
}

#[test]
fn the_copilot_pane_invites_an_instruction_before_the_first_turn() {
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);
    app.wf.focus = WorkflowFocus::Copilot;

    let screen = render(&mut app);

    assert!(screen.contains("add a Slack step"), "{screen}");
}

#[test]
fn the_sidebar_advertises_the_key_that_reaches_the_copilot() {
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);
    app.wf.focus = WorkflowFocus::Canvas;

    let screen = render(&mut app);

    // The copilot is no longer a column that can point at itself, so the
    // binding has to be named where the operator is. `c` is it (see
    // `keys/workflows.rs`); Tab is deliberately unbound in this tab and would
    // leave it entirely, so nothing may suggest it.
    assert!(screen.contains("c copilot"), "{screen}");
    assert!(!screen.contains("Tab to type"), "{screen}");
}

#[test]
fn a_copilot_thread_is_drawn_with_a_marker_per_kind_of_line() {
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);
    app.wf.focus = WorkflowFocus::Copilot;
    app.wf.draft = crate::ui::composer::insert_at("", 0, "add a step");
    app.submit_copilot().expect("turn");
    app.copilot_status("nightly", "applying ops".into());
    app.copilot_finished("nightly", "done".into(), vec!["+ node notify".into()], None);

    let screen = render(&mut app);

    assert!(screen.contains("❯ add a step"), "the instruction: {screen}");
    assert!(screen.contains("± + node notify"), "the change: {screen}");
    assert!(screen.contains("⏺ done"), "the reply: {screen}");
}

#[test]
fn the_focused_pane_is_the_one_with_the_highlighted_border() {
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);

    app.wf.focus = WorkflowFocus::Copilot;
    let screen = render(&mut app);

    // The composer's caret only renders in the focused pane, which is the
    // visible consequence of focus this test can read.
    assert!(screen.contains("⏎ send"), "{screen}");
}

#[test]
fn the_footer_teaches_this_tabs_bindings_rather_than_the_agents_tabs() {
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);

    let screen = render(&mut app);

    assert!(
        screen.contains("c copilot"),
        "the copilot is reached with c, not Tab: {screen}"
    );
    assert!(screen.contains("i inspect"), "{screen}");
    assert!(
        !screen.contains("⌥A answer"),
        "the Sessions-tab steering keys do nothing here"
    );
}

#[test]
fn tab_leaves_the_view_rather_than_cycling_panes_inside_it() {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);
    let workflows = app.tab_index;

    app.on_event(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)));

    assert_ne!(
        app.tab_index, workflows,
        "Tab belongs to the top-level view ring; a tab inside a tab would trap it"
    );
}

#[test]
fn enter_opens_the_graph_and_esc_unwinds_one_level_at_a_time() {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);
    let press = |app: &mut App, code| {
        app.on_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    };
    assert_eq!(app.wf_focus(), WorkflowFocus::Sidebar);

    press(&mut app, KeyCode::Enter);
    assert_eq!(app.wf_focus(), WorkflowFocus::Canvas);

    press(&mut app, KeyCode::Char('c'));
    assert_eq!(app.wf_focus(), WorkflowFocus::Copilot);

    press(&mut app, KeyCode::Esc);
    assert_eq!(app.wf_focus(), WorkflowFocus::Canvas);
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.wf_focus(), WorkflowFocus::Sidebar);
}

#[test]
fn a_digit_jumps_straight_to_that_workflow_and_opens_it() {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    let (_home, mut app) = app_with(&[diamond("first"), diamond("second")], &[]);

    app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char('2'),
        KeyModifiers::NONE,
    )));

    assert_eq!(
        app.workflow_summaries()[app.selected_workflow_index()].id,
        "second"
    );
    assert_eq!(
        app.wf_focus(),
        WorkflowFocus::Canvas,
        "a digit opens the page it jumps to, as it does on every other nav"
    );
}

#[test]
fn a_stray_letter_in_the_sidebar_does_not_fire_a_content_action() {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);

    app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char('i'),
        KeyModifiers::NONE,
    )));

    assert!(
        !app.wf_inspector_open(),
        "the inspector belongs to the canvas; the menu swallows its letters"
    );
}

#[test]
fn every_view_draws_at_any_terminal_width() {
    // What the old three-column layout could not do: on an 80-wide terminal it
    // left the canvas about twelve columns, less than one node box.
    for (width, height) in [(80, 24), (120, 30), (200, 50)] {
        let (_home, mut app) = app_with(&[diamond("nightly")], &[]);

        let graph = render_sized(&mut app, width, height);
        assert!(graph.contains("▶ Start"), "{width}x{height}: {graph}");

        app.wf.inspector_open = true;
        assert!(render_sized(&mut app, width, height).contains("trigger_kind"));

        app.wf.focus = WorkflowFocus::Copilot;
        assert!(render_sized(&mut app, width, height).contains("Copilot"));
    }
}

#[test]
fn a_terminal_too_small_to_draw_anything_does_not_panic() {
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);

    render_sized(&mut app, 20, 6);
}

mod layout_tests;

/// A trigger into a loop whose body is a four-step chain that returns to it —
/// the shape every "keep going until it is done" workflow has.
fn looping(id: &str) -> WorkflowRecord {
    WorkflowRecord {
        id: id.to_string(),
        name: format!("{id} loop"),
        description: String::new(),
        enabled: true,
        defaults: Default::default(),
        graph: serde_json::from_value(json!({
            "name": id,
            "nodes": [
                { "id": "start", "kind": "trigger", "name": "Start",
                  "config": { "trigger_kind": "manual" } },
                { "id": "watch", "kind": "loop", "name": "Until it settles",
                  "config": { "max_iterations": 8 } },
                { "id": "read", "kind": "tool_call", "name": "Read the state",
                  "config": { "slug": "medulla:shell" } },
                { "id": "fix", "kind": "agent", "name": "Fix it",
                  "config": { "prompt": "go" } },
                { "id": "again", "kind": "transform", "name": "Round again",
                  "config": { "expression": "=." } },
                { "id": "report", "kind": "transform", "name": "Report",
                  "config": { "expression": "=." } },
            ],
            "edges": [
                { "from_node": "start", "to_node": "watch" },
                { "from_node": "watch", "from_port": "body", "to_node": "read" },
                { "from_node": "watch", "from_port": "done", "to_node": "report" },
                { "from_node": "read", "to_node": "fix" },
                { "from_node": "fix", "to_node": "again" },
                // The arm that closes the loop.
                { "from_node": "again", "to_node": "watch" },
            ],
        }))
        .expect("graph parses"),
        source_path: None,
    }
}

#[test]
fn a_loop_folds_to_its_own_depth_rather_than_one_band_per_step() {
    let (_home, mut app) = app_with(&[looping("babysit")], &[]);
    render(&mut app);

    // Five columns of plan — start, the loop, and its three body steps, with
    // `Report` sharing the loop's own next column. The closing arm is not a
    // sixth step and must not rank as one.
    assert_eq!(app.workflow_layout().layers, 5);
    assert!(
        app.folded_rows() <= 6,
        "a six-node loop folded onto {} rows: {:?}",
        app.folded_rows(),
        app.band_heights()
    );
}

#[test]
fn a_node_name_is_never_broken_up_by_a_wire_crossing_it() {
    let (_home, mut app) = app_with(&[looping("babysit")], &[]);

    let screen = render(&mut app);

    // The loop's closing arm crosses this name's row on its way back. Its own
    // spaces used to resolve as the wire's box-drawing character.
    assert!(
        screen.contains("↺ Until it settles"),
        "the name is broken up: {screen}"
    );
    assert!(
        !screen.contains("↺─Until"),
        "a wire was drawn through the label: {screen}"
    );
}

#[test]
fn a_loops_closing_arm_comes_back_into_the_step_it_repeats() {
    let (_home, mut app) = app_with(&[looping("babysit")], &[]);

    let screen = render(&mut app);
    let row = screen
        .lines()
        .find(|line| line.contains("↺ Until it settles"))
        .expect("the loop is on some row");

    // Into the trailing edge — the side the work left by — rather than crossing
    // the whole box to arrive at its front, which is what routing the return
    // arm like a forward edge did.
    assert!(
        row.contains("↺ Until it settles ◀"),
        "the return arm does not arrive at the trailing edge: {row}"
    );
}

#[test]
fn a_fold_turns_against_the_frame_rather_than_costing_a_column() {
    let (_home, mut app) = app_with(&[looping("babysit")], &[]);

    // Narrow enough that the last column of a band ends flush against the panel
    // border, which is where the wire leaving it has nowhere to turn.
    let screen = render_sized(&mut app, 74, 30);

    // The width goes on the plan: reserving a right margin for that turn cost
    // roughly a whole layer per band, and a graph that folds after every step
    // is harder to read than one whose last fold turns against the frame.
    assert!(
        app.layers_per_band() > 1,
        "a margin ate the second column: {} layers per band",
        app.layers_per_band()
    );
    // And the turn is still drawn, rather than being clipped off the canvas.
    assert!(
        screen.lines().any(|line| line.contains('╮')),
        "the fold has no visible turn: {screen}"
    );
}

#[test]
fn a_port_name_in_the_gutter_leaves_the_arrowhead_its_cell() {
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);

    let screen = render(&mut app);

    // `false` is the longest port name in ordinary use and exactly fills the
    // gutter. Written a cell wider it lands on the arrowhead — and since the
    // label is painted after the arrow, it simply replaces it, leaving an arm
    // that reads as a wire which never arrived.
    assert!(
        screen.contains("false▶"),
        "the false arm lost its arrowhead: {screen}"
    );
    assert!(screen.contains("true"), "and the true arm is still named");
}

#[test]
fn a_folded_arms_port_name_stands_clear_of_the_wire() {
    let (_home, mut app) = app_with(&[looping("babysit")], &[]);

    // Narrow enough that the loop's body arm folds onto the next band.
    let screen = render_sized(&mut app, 74, 30);
    let row = screen
        .lines()
        .find(|line| line.contains("body"))
        .expect("the body arm is named somewhere");
    let at = row.find("body").expect("just found it");

    // A blank either side, so the name reads as a caption on the wire rather
    // than as part of the drawing: `╭body────────` ran the corner, the name and
    // the run together into one token.
    assert_eq!(&row[at - 1..at], " ", "no blank before the name: {row}");
    assert_eq!(&row[at + 4..at + 5], " ", "no blank after the name: {row}");
}
