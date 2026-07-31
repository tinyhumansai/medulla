//! Feature tests for the Workflows tab: what the rail lists, what the canvas
//! draws, and what the keys do.
//!
//! Every test injects a temp Medulla home *and* a store scoped to it, so the tab
//! reads workflows this test wrote and never the developer's own — the layered
//! store a real session resolves also includes the current directory's
//! `.medulla/workflows`, which is right in use and wrong under test.

#![cfg(feature = "workflows")]

use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::workflows::{FileWorkflowStore, WorkflowStore};
use medulla_tui::ui::app::{App, TABS};
use serde_json::json;

/// A store over `home` alone.
fn store(home: &std::path::Path) -> Arc<dyn WorkflowStore> {
    Arc::new(FileWorkflowStore::new(
        vec![home.join("workflows")],
        home.join("state").join("workflows").join("runs"),
    ))
}

/// An app parked on the Workflows tab with `home` as its Medulla home.
fn workflows_app(home: &std::path::Path) -> App {
    let rt = Arc::new(MockRuntime::demo());
    let mut app = App::new(rt, LoadedConfig::defaults("medulla.tui.json".into()));
    app.set_medulla_home(home.to_path_buf());
    app.set_workflow_store(store(home));
    app.tab_index = TABS
        .iter()
        .position(|tab| *tab == "Workflows")
        .expect("Workflows is a tab");
    app
}

/// Install a workflow into the home store the tab reads.
fn install(home: &std::path::Path, id: &str, name: &str, enabled: bool) {
    let document = json!({
        "id": id,
        "name": name,
        "description": "does the thing",
        "enabled": enabled,
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "work", "kind": "agent", "name": "Work",
              "config": { "prompt": "do it" } }
        ],
        "edges": [{ "from_node": "t", "to_node": "work" }]
    })
    .to_string();
    let record = medulla::workflows::store::parse_workflow(&document, id).expect("valid fixture");
    store(home).save(&record).expect("installs");
}

/// Render the app and return the screen as text.
///
/// Wide, because the tab is three panes and an 80-column dump would clip the
/// canvas out of every assertion.
fn rendered(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(150, 34)).expect("test terminal");
    terminal.draw(|f| app.draw(f)).expect("draws");
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>()
}

fn press(app: &mut App, code: KeyCode) {
    app.on_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

#[test]
fn the_tab_lists_the_workflows_installed_on_this_machine() {
    let home = tempfile::tempdir().unwrap();
    install(home.path(), "sweep", "Nightly sweep", true);
    install(home.path(), "triage", "Triage", true);
    let mut app = workflows_app(home.path());

    app.reload_workflows();

    assert_eq!(app.workflow_row_count(), 2);
    let screen = rendered(&mut app);
    assert!(screen.contains("Nightly sweep"), "{screen}");
    assert!(screen.contains("Triage"), "{screen}");
    assert!(
        !screen.contains("2 steps"),
        "the compact sidebar carries names only: {screen}"
    );
}

#[test]
fn an_empty_store_explains_how_to_get_a_workflow_rather_than_looking_broken() {
    let home = tempfile::tempdir().unwrap();
    let mut app = workflows_app(home.path());

    app.reload_workflows();

    assert_eq!(app.workflow_row_count(), 0);
    let screen = rendered(&mut app);
    assert!(screen.contains("No workflows installed"), "{screen}");
    assert!(
        screen.contains("copilot"),
        "the tab should point at the way to get one: {screen}"
    );
}

#[test]
fn the_arrow_keys_move_the_selection_within_the_rail() {
    let home = tempfile::tempdir().unwrap();
    install(home.path(), "a-first", "Alpha", true);
    install(home.path(), "b-second", "Beta", true);
    let mut app = workflows_app(home.path());
    app.reload_workflows();

    press(&mut app, KeyCode::Down);
    assert_eq!(
        app.workflow_summaries()[app.selected_workflow_index()].name,
        "Beta"
    );
    let after_down = rendered(&mut app);

    press(&mut app, KeyCode::Up);
    assert_eq!(
        app.workflow_summaries()[app.selected_workflow_index()].name,
        "Alpha"
    );
    let after_up = rendered(&mut app);

    assert!(after_down.contains("Alpha") && after_down.contains("Beta"));
    assert!(after_up.contains("Alpha") && after_up.contains("Beta"));
}

#[test]
fn selecting_a_workflow_draws_its_graph_on_the_canvas() {
    let home = tempfile::tempdir().unwrap();
    install(home.path(), "sweep", "Nightly sweep", true);
    let mut app = workflows_app(home.path());

    app.reload_workflows();

    let screen = rendered(&mut app);
    assert!(screen.contains("▶ start"), "the trigger box: {screen}");
    assert!(screen.contains("✦ Work"), "the agent box: {screen}");
    assert!(
        screen.contains('▶'),
        "the edge between them ends in an arrow"
    );
}

#[test]
fn r_re_reads_the_store_so_a_workflow_written_while_the_app_is_open_appears() {
    let home = tempfile::tempdir().unwrap();
    install(home.path(), "sweep", "Nightly sweep", true);
    let mut app = workflows_app(home.path());
    app.reload_workflows();
    assert_eq!(app.workflow_row_count(), 1);

    // The usual way a workflow arrives: an agent or an editor writes the file.
    install(home.path(), "triage", "Triage", true);
    press(&mut app, KeyCode::Char('r'));

    assert_eq!(app.workflow_row_count(), 2);
    assert!(rendered(&mut app).contains("Triage"));
}

#[test]
fn a_disabled_workflow_is_still_listed_by_name() {
    let home = tempfile::tempdir().unwrap();
    install(home.path(), "paused", "Paused sweep", false);
    let mut app = workflows_app(home.path());

    app.reload_workflows();

    let screen = rendered(&mut app);
    assert!(screen.contains("Paused sweep"), "{screen}");
    assert!(
        !screen.contains("disabled"),
        "the sidebar intentionally carries names only: {screen}"
    );
}

#[test]
fn asking_to_run_a_disabled_workflow_refuses_rather_than_running_it() {
    let home = tempfile::tempdir().unwrap();
    install(home.path(), "paused", "Paused sweep", false);
    let mut app = workflows_app(home.path());
    app.reload_workflows();

    press(&mut app, KeyCode::Char('x'));

    assert!(
        app.status().contains("disabled"),
        "the status line should say why nothing happened: {}",
        app.status()
    );
}

#[test]
fn the_rail_shows_the_selected_workflows_run_history() {
    let home = tempfile::tempdir().unwrap();
    install(home.path(), "sweep", "Nightly sweep", true);
    let mut record = medulla::workflows::new_run_record("run-7", "sweep", 100);
    record.status = medulla::workflows::RunStatus::Succeeded;
    store(home.path()).record_run(&record).expect("records");

    let mut app = workflows_app(home.path());
    app.reload_workflows();

    let screen = rendered(&mut app);
    assert!(
        screen.contains("succeeded"),
        "the outcome is the point of the history: {screen}"
    );
    assert!(
        screen.contains('7'),
        "enough of the run id to find it again: {screen}"
    );
}

#[test]
fn a_workflow_that_has_never_run_says_so_rather_than_showing_nothing() {
    let home = tempfile::tempdir().unwrap();
    install(home.path(), "sweep", "Nightly sweep", true);
    let mut app = workflows_app(home.path());

    app.reload_workflows();

    assert!(rendered(&mut app).contains("no runs yet"));
}

#[test]
fn selecting_a_run_overlays_it_on_the_graph() {
    let home = tempfile::tempdir().unwrap();
    install(home.path(), "sweep", "Nightly sweep", true);
    let mut record = medulla::workflows::new_run_record("run-7", "sweep", 100);
    record.status = medulla::workflows::RunStatus::Succeeded;
    record.steps = vec![medulla::workflows::RunStep {
        node_id: "t".into(),
        status: "success".into(),
        duration_ms: 4,
        input: None,
        output: None,
        diagnostics: Vec::new(),
    }];
    store(home.path()).record_run(&record).expect("records");
    let mut app = workflows_app(home.path());
    app.reload_workflows();

    // Down from the workflow row lands on its first run.
    press(&mut app, KeyCode::Down);

    let screen = rendered(&mut app);
    assert!(
        screen.contains("run "),
        "the canvas names the overlaid run: {screen}"
    );
    assert!(
        screen.contains('✓'),
        "the step it reached is marked: {screen}"
    );
}

#[test]
fn the_workflows_tab_is_reachable_from_the_tab_bar() {
    let home = tempfile::tempdir().unwrap();
    let mut app = workflows_app(home.path());

    // The bar lists it, so an operator finds the feature without being told.
    assert!(rendered(&mut app).contains("Workflows"));
}

/// Install a workflow that declares inputs, so pressing run has something to
/// prompt for.
fn install_parameterized(home: &std::path::Path, id: &str) {
    let document = json!({
        "id": id,
        "name": "Review a repo",
        "description": "reviews the named repo",
        "enabled": true,
        "inputs": [
            { "name": "repo", "type": "string", "required": true,
              "description": "Repo to review" },
            { "name": "depth", "type": "number", "default": 3 }
        ],
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "work", "kind": "agent", "name": "Work",
              "config": { "prompt": "review =inputs.repo" } }
        ],
        "edges": [{ "from_node": "t", "to_node": "work" }]
    })
    .to_string();
    let record = medulla::workflows::store::parse_workflow(&document, id).expect("valid fixture");
    store(home).save(&record).expect("installs");
}

/// Type each character of `text` into the open prompt.
fn type_text(app: &mut App, text: &str) {
    for ch in text.chars() {
        press(app, KeyCode::Char(ch));
    }
}

#[test]
fn running_a_workflow_with_declared_inputs_prompts_for_each_field() {
    let home = tempfile::tempdir().unwrap();
    install_parameterized(home.path(), "review");
    let mut app = workflows_app(home.path());
    app.reload_workflows();

    press(&mut app, KeyCode::Char('x'));

    // The first field, named and described, rather than the run starting blind.
    let screen = rendered(&mut app);
    assert!(screen.contains("repo"), "{screen}");
    assert!(screen.contains("Repo to review"), "{screen}");

    type_text(&mut app, "acme/api");
    press(&mut app, KeyCode::Enter);

    // The second field, pre-filled with its declared default so accepting it is
    // one keypress.
    let screen = rendered(&mut app);
    assert!(screen.contains("depth"), "{screen}");
    assert!(
        screen.contains('3'),
        "the default should be pre-filled: {screen}"
    );
}

#[test]
fn a_workflow_with_no_declared_inputs_runs_without_prompting() {
    let home = tempfile::tempdir().unwrap();
    install(home.path(), "sweep", "Nightly sweep", true);
    let mut app = workflows_app(home.path());
    app.reload_workflows();

    press(&mut app, KeyCode::Char('x'));

    let screen = rendered(&mut app);
    assert!(
        screen.contains("Running"),
        "an unparameterized workflow should dispatch straight away: {screen}"
    );
}

#[test]
fn escaping_an_input_prompt_abandons_the_whole_run() {
    let home = tempfile::tempdir().unwrap();
    install_parameterized(home.path(), "review");
    let mut app = workflows_app(home.path());
    app.reload_workflows();

    press(&mut app, KeyCode::Char('x'));
    type_text(&mut app, "acme/api");
    press(&mut app, KeyCode::Enter);
    // Now on the second field; back out.
    press(&mut app, KeyCode::Esc);

    let screen = rendered(&mut app);
    assert!(
        !screen.contains("depth"),
        "the prompt chain should be gone: {screen}"
    );
    assert!(
        !screen.contains("Running"),
        "cancelling a field must not start the run anyway: {screen}"
    );
}

#[test]
fn a_value_of_the_wrong_type_re_asks_the_same_field() {
    let home = tempfile::tempdir().unwrap();
    install_parameterized(home.path(), "review");
    let mut app = workflows_app(home.path());
    app.reload_workflows();

    press(&mut app, KeyCode::Char('x'));
    type_text(&mut app, "acme/api");
    press(&mut app, KeyCode::Enter);

    // `depth` is declared a number; clear the pre-filled default and type text.
    for _ in 0..4 {
        press(&mut app, KeyCode::Backspace);
    }
    type_text(&mut app, "deep");
    press(&mut app, KeyCode::Enter);

    let screen = rendered(&mut app);
    assert!(
        screen.contains("depth"),
        "the same field should be asked again rather than the set being dropped: {screen}"
    );
    assert!(
        screen.contains("expects a number"),
        "the reason should say what was wrong: {screen}"
    );
}
