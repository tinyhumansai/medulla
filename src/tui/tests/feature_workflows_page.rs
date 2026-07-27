//! Feature tests for Routing > Workflows: what the page lists, what it draws,
//! and what the keys do.
//!
//! Every test injects a temp Medulla home via `set_medulla_home`, so the page
//! reads workflows this test wrote and never the developer's own.

#![cfg(feature = "workflows")]

use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::workflows::{FileWorkflowStore, WorkflowStore};
use medulla_tui::ui::app::App;
use serde_json::json;

/// An app parked on the Workflows page with `home` as its Medulla home.
fn workflows_app(home: &std::path::Path) -> App {
    let rt = Arc::new(MockRuntime::demo());
    let mut app = App::new(rt, LoadedConfig::defaults("medulla.tui.json".into()));
    app.set_medulla_home(home.to_path_buf());
    app.focus_routing_subpage("Workflows");
    app
}

/// Install a workflow into the home store the page reads.
fn install(home: &std::path::Path, id: &str, name: &str, enabled: bool) {
    let store = FileWorkflowStore::new(
        vec![home.join("workflows")],
        home.join("state").join("workflows").join("runs"),
    );
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
    store.save(&record).expect("installs");
}

/// Render the app and return the screen as text.
fn rendered(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("test terminal");
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
fn the_page_lists_the_workflows_installed_on_this_machine() {
    let home = tempfile::tempdir().unwrap();
    install(home.path(), "sweep", "Nightly sweep", true);
    install(home.path(), "triage", "Triage", true);
    let mut app = workflows_app(home.path());

    app.reload_workflows();

    assert_eq!(app.workflow_row_count(), 2);
    let screen = rendered(&mut app);
    assert!(screen.contains("Nightly sweep"), "{screen}");
    assert!(screen.contains("Triage"), "{screen}");
    // The detail line should say enough to choose between them.
    assert!(screen.contains("2 steps"), "{screen}");
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
        screen.contains("medulla workflow catalog"),
        "the page should point at the way to build one: {screen}"
    );
}

#[test]
fn the_arrow_keys_move_the_selection_within_the_list() {
    let home = tempfile::tempdir().unwrap();
    install(home.path(), "a-first", "Alpha", true);
    install(home.path(), "b-second", "Beta", true);
    let mut app = workflows_app(home.path());
    app.reload_workflows();

    press(&mut app, KeyCode::Down);
    let after_down = rendered(&mut app);
    press(&mut app, KeyCode::Up);
    let after_up = rendered(&mut app);

    // Both workflows stay listed; only the highlight moves, which the text dump
    // cannot see — so assert the count is stable and the app did not panic or
    // drop a row on either keypress.
    assert_eq!(app.workflow_row_count(), 2);
    assert!(after_down.contains("Alpha") && after_down.contains("Beta"));
    assert!(after_up.contains("Alpha") && after_up.contains("Beta"));
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
fn a_disabled_workflow_is_listed_and_says_why_it_will_not_run() {
    let home = tempfile::tempdir().unwrap();
    install(home.path(), "paused", "Paused sweep", false);
    let mut app = workflows_app(home.path());

    app.reload_workflows();

    let screen = rendered(&mut app);
    assert!(screen.contains("Paused sweep"), "{screen}");
    assert!(
        screen.contains("disabled"),
        "an operator should see why it will not run: {screen}"
    );
}

#[test]
fn pressing_enter_on_a_disabled_workflow_refuses_rather_than_running_it() {
    let home = tempfile::tempdir().unwrap();
    install(home.path(), "paused", "Paused sweep", false);
    let mut app = workflows_app(home.path());
    app.reload_workflows();

    press(&mut app, KeyCode::Enter);

    assert!(
        app.status().contains("disabled"),
        "the status line should say why nothing happened: {}",
        app.status()
    );
}

#[test]
fn the_page_shows_the_selected_workflows_run_history() {
    let home = tempfile::tempdir().unwrap();
    install(home.path(), "sweep", "Nightly sweep", true);
    let store = FileWorkflowStore::new(
        vec![home.path().join("workflows")],
        home.path().join("state").join("workflows").join("runs"),
    );
    let mut record = medulla::workflows::new_run_record("run-7", "sweep", 100);
    record.status = medulla::workflows::RunStatus::Succeeded;
    store.record_run(&record).expect("records");

    let mut app = workflows_app(home.path());
    app.reload_workflows();

    let screen = rendered(&mut app);
    assert!(screen.contains("recent runs"), "{screen}");
    assert!(screen.contains("run-7"), "{screen}");
    assert!(
        screen.contains("succeeded"),
        "the outcome is the point of the history: {screen}"
    );
}

#[test]
fn a_workflow_that_has_never_run_says_so_rather_than_showing_nothing() {
    let home = tempfile::tempdir().unwrap();
    install(home.path(), "sweep", "Nightly sweep", true);
    let mut app = workflows_app(home.path());

    app.reload_workflows();

    assert!(rendered(&mut app).contains("No runs yet"));
}

#[test]
fn the_workflows_page_is_reachable_from_the_routing_nav() {
    let home = tempfile::tempdir().unwrap();
    let mut app = workflows_app(home.path());

    // The nav lists it, so an operator can find the feature without being told
    // it exists.
    assert!(rendered(&mut app).contains("Workflows"));
}
