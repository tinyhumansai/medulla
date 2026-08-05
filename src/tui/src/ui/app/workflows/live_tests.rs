//! What the live-run buffer keeps for a run this TUI started.
use std::sync::Arc;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;

use crate::ui::app::App;

fn app() -> App {
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()))
}

#[test]
fn frames_are_kept_under_the_node_that_emitted_them() {
    let mut app = app();
    app.workflow_run_started("review", "run-1");
    app.workflow_run_output("run-1", "implement", "writing".into());
    app.workflow_run_output("run-1", "verify", "cargo test".into());

    let run = app.live_runs.get("run-1").expect("a tracked run");
    assert_eq!(run.frames("implement"), ["writing"]);
    assert_eq!(run.frames("verify"), ["cargo test"]);
    // A node that has said nothing has nothing, rather than the run's
    // frames spilling into every box on the graph.
    assert!(run.frames("report").is_empty());
}

#[test]
fn a_frame_for_an_untracked_run_is_dropped_rather_than_creating_one() {
    let mut app = app();
    app.workflow_run_output("run-gone", "implement", "writing".into());

    assert!(app.live_runs.is_empty());
}

#[test]
fn a_settled_run_keeps_its_frames_and_stops_claiming_to_be_live() {
    let mut app = app();
    app.workflow_run_started("review", "run-1");
    app.workflow_run_output("run-1", "implement", "writing".into());
    app.workflow_run_finished("run-1");

    let run = app.live_runs.get("run-1").expect("a tracked run");
    assert!(!run.running);
    assert_eq!(run.frames("implement"), ["writing"]);
}

#[test]
fn starting_a_second_run_of_one_workflow_replaces_the_settled_picture() {
    let mut app = app();
    app.workflow_run_started("review", "run-1");
    app.workflow_run_finished("run-1");
    app.workflow_run_started("review", "run-2");

    // The finished run's frames would otherwise sit under the same nodes as
    // the new run's, and the pane draws one node at a time.
    assert!(!app.live_runs.contains_key("run-1"));
    assert!(app.live_runs.contains_key("run-2"));
}
