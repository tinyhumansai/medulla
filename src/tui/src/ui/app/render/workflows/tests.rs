//! Rendering tests for the Workflows tab.
//!
//! Every test draws the whole tab onto a [`TestBackend`] and reads the resulting
//! screen, because the thing under test is what an operator sees: that the three
//! panes are all present, that the graph is drawn as boxes joined by wires, and
//! that a run overlaid on it marks the steps it reached.

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
fn the_tab_draws_all_three_panes() {
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);

    let screen = render(&mut app);

    assert!(screen.contains("Workflows · 1"), "the rail is missing");
    assert!(
        screen.contains("nightly sweep"),
        "the canvas title is missing"
    );
    assert!(screen.contains("Copilot"), "the copilot pane is missing");
}

#[test]
fn the_graph_is_drawn_as_boxes_joined_by_wires() {
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);

    let screen = render(&mut app);

    assert!(screen.contains("▶ Start"), "the trigger box is missing");
    assert!(screen.contains("◆ Check"), "the condition box is missing");
    assert!(
        screen.contains('╭') && screen.contains('╯'),
        "nodes are drawn as boxes"
    );
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
            diagnostics: Vec::new(),
        }],
        pending_approvals: Vec::new(),
        error: None,
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
                diagnostics: Vec::new(),
            },
            RunStep {
                node_id: "check".into(),
                status: "failed".into(),
                duration_ms: 4,
                diagnostics: Vec::new(),
            },
        ],
        pending_approvals: Vec::new(),
        error: Some("boom".into()),
    };
    let (_home, mut app) = app_with(&[diamond("nightly")], &[run]);

    app.move_workflow_rail(false);
    let screen = render(&mut app);

    assert!(screen.contains("run "), "the title names the overlaid run");
    assert!(screen.contains('✓'), "a step that passed is marked");
    assert!(screen.contains('✗'), "the step that failed is marked");
}

#[test]
fn the_inspector_is_a_strip_until_it_is_opened() {
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);

    let closed = render(&mut app);
    assert!(closed.contains("i expands"), "{closed}");

    app.wf.inspector_open = true;
    let open = render(&mut app);
    assert!(
        open.contains("trigger_kind"),
        "the open inspector shows the node's declaration"
    );
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

    let screen = render(&mut app);

    assert!(screen.contains("add a Slack step"), "{screen}");
}

#[test]
fn the_unfocused_copilot_pane_advertises_the_key_that_actually_focuses_it() {
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);
    // Sidebar is the default focus, so the copilot pane starts unfocused.
    assert_eq!(app.wf_focus(), WorkflowFocus::Sidebar);

    let screen = render(&mut app);

    // `c` is the binding (see `keys/workflows.rs`); Tab is deliberately
    // unbound in this tab and would leave it instead of focusing the
    // composer, so the hint must never name it.
    assert!(screen.contains("c to type"), "{screen}");
    assert!(!screen.contains("Tab to type"), "{screen}");
}

#[test]
fn a_copilot_thread_is_drawn_with_a_marker_per_kind_of_line() {
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);
    app.wf.draft = crate::ui::composer::insert_at("", 0, "add a step");
    app.submit_copilot().expect("turn");
    app.copilot_status("nightly", "applying ops".into());
    app.copilot_finished("nightly", "done".into(), vec!["+ node notify".into()]);

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
        "the Agents-tab steering keys do nothing here"
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
fn a_narrow_terminal_still_draws_every_pane_without_panicking() {
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);

    let screen = render_sized(&mut app, 80, 24);

    assert!(screen.contains("Workflows"));
    assert!(screen.contains("Copilot"));
}

#[test]
fn a_terminal_too_small_to_draw_anything_does_not_panic() {
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);

    render_sized(&mut app, 20, 6);
}

#[test]
fn a_graph_wider_than_the_canvas_scrolls_to_the_cursor() {
    let mut long = diamond("long");
    long.graph = serde_json::from_value(json!({
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
    let (_home, mut app) = app_with(&[long], &[]);

    let start = render(&mut app);
    assert!(start.contains("Step 0"));
    assert!(!start.contains("Step 11"), "the far end is off screen");

    for _ in 0..11 {
        app.move_graph_cursor(Move::Forward);
    }
    let end = render(&mut app);
    assert!(
        end.contains("Step 11"),
        "the cursor pulled the canvas along"
    );
}
