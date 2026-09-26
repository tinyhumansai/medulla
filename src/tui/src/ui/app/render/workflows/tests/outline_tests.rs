//! What the outline draws, when it is chosen, and what it says about steps that
//! turn out to be one step written several times.

use super::*;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

/// A chain long enough to fold, with names too long for a column, and a gate
/// whose arms are four copies of one reporting step.
///
/// Modelled on the workflow that motivated the view: a spine of wordy steps with
/// exits hanging off it that differ only in the reason they pass along.
fn issue_shaped(id: &str) -> WorkflowRecord {
    let report = |name: &str, why: &str| {
        json!({
            "id": format!("report_{why}"),
            "kind": "tool_call",
            "name": name,
            "config": {
                "slug": "medulla:shell",
                "args": { "language": "shell", "script": "jq -n '{done:true}'", "env": { "WHY": why } },
            },
        })
    };
    let mut record = diamond(id);
    record.name = "Implement a GitHub issue".into();
    record.graph = serde_json::from_value(json!({
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "Implement an issue",
              "config": { "trigger_kind": "manual" } },
            { "id": "resolve", "kind": "tool_call", "name": "Resolve repo from remotes",
              "config": { "slug": "medulla:shell", "args": { "language": "shell", "script": "git remote -v" } } },
            { "id": "collect", "kind": "tool_call", "name": "Read the issue and its discussion",
              "config": { "slug": "medulla:shell", "args": { "language": "shell", "script": "gh issue view" } } },
            { "id": "actionable", "kind": "condition", "name": "Open, and nothing already in flight?",
              "config": { "expression": "=.open" } },
            { "id": "implement", "kind": "agent", "name": "Implement it and open the PR",
              "config": { "prompt": "=\"Implement issue #\\(.inputs.issue).\\n\\nOpen a pull request.\"" } },
            { "id": "opened", "kind": "condition", "name": "Was a PR opened?",
              "config": { "expression": "=.found" } },
            report("Final report (skip)", "skip"),
            report("Final report (nopr)", "nopr"),
            report("Final report (done)", "done"),
            report("Final report (late)", "late"),
        ],
        "edges": [
            { "from_node": "t", "to_node": "resolve" },
            { "from_node": "resolve", "to_node": "collect" },
            { "from_node": "collect", "to_node": "actionable" },
            { "from_node": "actionable", "from_port": "true", "to_node": "implement" },
            { "from_node": "actionable", "from_port": "false", "to_node": "report_skip" },
            { "from_node": "implement", "to_node": "opened" },
            { "from_node": "opened", "from_port": "false", "to_node": "report_nopr" },
            { "from_node": "opened", "from_port": "true", "to_node": "report_done" },
            { "from_node": "collect", "from_port": "late", "to_node": "report_late" },
        ],
    }))
    .expect("graph parses");
    record
}

#[test]
fn a_folded_chain_is_drawn_as_an_outline_with_its_names_intact() {
    let (_home, mut app) = app_with(&[issue_shaped("issue")], &[]);

    // 100 columns is not enough to lay this chain across the pane, which is
    // exactly when the wire canvas starts clipping names to a column width.
    let screen = render_sized(&mut app, 100, 30);

    assert!(app.outline_view(), "a folded chain reads better as a list");
    assert!(
        screen.contains("Read the issue and its discussion"),
        "no name is clipped at any width: {screen}"
    );
    assert!(
        screen.contains("├ false → ") || screen.contains("╰ false → "),
        "a gate's arms are drawn as the choice they are: {screen}"
    );
    for name in [
        "Resolve repo from remotes",
        "Open, and nothing already in flight?",
        "Implement it and open the PR",
    ] {
        assert!(screen.contains(name), "{name} is drawn in full: {screen}");
    }
}

#[test]
fn a_graph_that_fits_keeps_its_wires() {
    // The outline is for a chain the canvas would have to fold. A graph that
    // fits is already one readable row of boxes, and the wires say more about it
    // than an indent can.
    let (_home, mut app) = app_with(&[diamond("nightly")], &[]);

    render_sized(&mut app, 160, 40);

    assert!(!app.outline_view());
}

#[test]
fn v_pins_the_layout_the_operator_wants() {
    let (_home, mut app) = app_with(&[issue_shaped("issue")], &[]);
    render_sized(&mut app, 100, 30);
    assert!(app.outline_view());

    app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )));
    app.on_event(Event::Key(KeyEvent::new(
        KeyCode::Char('v'),
        KeyModifiers::NONE,
    )));

    assert!(!app.outline_view(), "v swaps to the wires");
    let wires = render_sized(&mut app, 100, 30);
    assert!(wires.contains("▶ Implement an issue"), "{wires}");
}

#[test]
fn steps_that_are_one_step_under_several_names_say_so() {
    let (_home, mut app) = app_with(&[issue_shaped("issue")], &[]);

    let screen = render_sized(&mut app, 120, 34);

    assert!(
        screen.contains("Final report ×4"),
        "four copies of one step are drawn as the one step they are: {screen}"
    );
    assert!(
        screen.contains("args.env.WHY"),
        "and what actually differs between them is named: {screen}"
    );
}

#[test]
fn the_step_preview_takes_the_room_it_needs_and_no_more() {
    let (_home, mut app) = app_with(&[issue_shaped("issue")], &[]);

    // The cursor starts on the trigger, which has three lines to its name. The
    // graph beside it has fifteen rows to draw, and used to get half the pane
    // whatever either of them had to say.
    let screen = render_sized(&mut app, 120, 34);
    let rows: Vec<&str> = screen.lines().collect();
    let preview = rows
        .iter()
        .position(|row| row.contains("wheel/Page scroll"))
        .expect("the preview is on screen");
    let graph = rows
        .iter()
        .position(|row| row.contains("Implement a GitHub issue"))
        .expect("the graph is on screen");

    assert!(
        preview - graph > rows.len() / 2,
        "the graph keeps most of the pane: preview starts at {preview} of {}",
        rows.len()
    );
    assert!(
        screen.contains("Was a PR opened?"),
        "and the whole graph is on screen rather than scrolled: {screen}"
    );
}

#[test]
fn an_interpolated_prompt_is_read_as_prose_rather_than_as_a_program() {
    let (_home, mut app) = app_with(&[issue_shaped("issue")], &[]);
    // Onto the agent step, and into the inspector, which is where a long prompt
    // is actually read.
    app.wf.node_index = app
        .workflow_layout()
        .index_of("implement")
        .expect("the agent step is in the graph");
    app.wf.inspector_open = true;

    let screen = render_sized(&mut app, 120, 34);

    assert!(
        screen.contains("Implement issue #${inputs.issue}"),
        "the hole is named rather than shown as jq: {screen}"
    );
    assert!(
        !screen.contains("\\n"),
        "and the author's line breaks are line breaks: {screen}"
    );
}
