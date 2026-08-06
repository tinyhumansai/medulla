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

#[test]
fn starting_a_second_run_also_replaces_one_still_executing() {
    let mut app = app();
    app.workflow_run_started("review", "run-1");
    app.workflow_run_output("run-1", "implement", "old work".into());
    // No `workflow_run_finished`: run-1 is still going when run-2 starts.
    app.workflow_run_started("review", "run-2");

    // Both would otherwise be `running`, and `live_run_for_view` picks a
    // running run out of a `HashMap` — so the pane could draw either one.
    assert!(!app.live_runs.contains_key("run-1"));
    assert_eq!(app.live_runs.len(), 1);
    let run = app.live_runs.get("run-2").expect("the replacement run");
    assert!(run.frames("implement").is_empty());
}

#[test]
fn a_run_of_another_workflow_is_left_alone() {
    let mut app = app();
    app.workflow_run_started("review", "run-1");
    app.workflow_run_started("release", "run-2");

    assert!(app.live_runs.contains_key("run-1"));
    assert!(app.live_runs.contains_key("run-2"));
}

#[test]
fn a_node_keeps_only_its_newest_frames() {
    let mut app = app();
    app.workflow_run_started("review", "run-1");
    let overshoot = 5;
    for index in 0..super::MAX_FRAMES_PER_NODE + overshoot {
        app.workflow_run_output("run-1", "implement", format!("frame {index}"));
    }

    let run = app.live_runs.get("run-1").expect("a tracked run");
    let frames = run.frames("implement");
    assert_eq!(frames.len(), super::MAX_FRAMES_PER_NODE);
    // The window slid: the oldest frames went, the newest is the last one in.
    assert_eq!(frames[0], format!("frame {overshoot}"));
    assert_eq!(
        frames.last().map(String::as_str),
        Some(format!("frame {}", super::MAX_FRAMES_PER_NODE + overshoot - 1).as_str())
    );
}

#[test]
fn a_reported_run_becomes_the_same_shape_as_a_local_one() {
    use medulla::control_socket::{HarnessRun, HarnessRunFrame, HarnessRunStatus};

    let run = HarnessRun {
        run_id: "run-remote".into(),
        workflow_id: "review".into(),
        status: HarnessRunStatus::Succeeded,
        started_at: 0,
        updated_at: 1,
        detail: Some("done".into()),
        frames: vec![
            // No node: about the run itself, with no box on the graph to sit in.
            HarnessRunFrame {
                node: None,
                text: "started".into(),
            },
            HarnessRunFrame {
                node: Some("implement".into()),
                text: "writing".into(),
            },
            HarnessRunFrame {
                node: Some("implement".into()),
                text: "still writing".into(),
            },
        ],
    };

    let view = super::LiveRun::from_reported(&run);
    assert_eq!(view.workflow, "review");
    // Terminal on the wire means not live here.
    assert!(!view.running);
    assert_eq!(view.frames("implement"), ["writing", "still writing"]);
    assert!(view.frames("started").is_empty());
}

#[test]
fn a_reported_run_that_is_still_going_reads_as_live() {
    use medulla::control_socket::{HarnessRun, HarnessRunStatus};

    let run = HarnessRun {
        run_id: "run-remote".into(),
        workflow_id: "review".into(),
        status: HarnessRunStatus::Running,
        started_at: 0,
        updated_at: 1,
        detail: None,
        frames: Vec::new(),
    };

    assert!(super::LiveRun::from_reported(&run).running);
}

#[test]
fn the_selected_reported_run_beats_a_local_run_of_the_same_workflow() {
    use medulla::control_socket::{RunReport, RunStatusWire};

    let mut app = app();
    // The workflow-level fallback needs a selection to fall back *to*, which is
    // exactly the state that used to answer with the wrong run.
    app.workflows = vec![medulla::workflows::WorkflowSummary {
        id: "review".to_string(),
        name: "review".to_string(),
        description: String::new(),
        enabled: true,
        node_count: 1,
        trigger_kind: None,
        inputs: Vec::new(),
    }];
    app.workflow_index = 0;

    // A local run of the same workflow is live here...
    app.workflow_run_started("review", "run-local");
    app.workflow_run_output("run-local", "implement", "local frame".into());
    // ...and another process reports one the operator then selects from the rail.
    app.harness_runs.report(
        "grant-1",
        RunReport {
            run_id: "run-remote".to_string(),
            workflow_id: "review".to_string(),
            status: RunStatusWire::Running,
            node: Some("implement".to_string()),
            detail: Some("remote frame".to_string()),
        },
    );
    app.wf.overlay = Some("run-remote".to_string());

    let view = app.live_run_view().expect("the selected run");
    assert_eq!(view.frames("implement"), ["remote frame"]);
}

#[test]
fn an_overlay_naming_nothing_still_falls_back_to_the_workflows_live_run() {
    let mut app = app();
    app.workflows = vec![medulla::workflows::WorkflowSummary {
        id: "review".to_string(),
        name: "review".to_string(),
        description: String::new(),
        enabled: true,
        node_count: 1,
        trigger_kind: None,
        inputs: Vec::new(),
    }];
    app.workflow_index = 0;
    app.workflow_run_started("review", "run-local");
    app.workflow_run_output("run-local", "implement", "local frame".into());
    // Pressing `x` before the run row exists: the id names neither source.
    app.wf.overlay = Some("run-gone".to_string());

    let view = app.live_run_view().expect("the workflow's live run");
    assert_eq!(view.frames("implement"), ["local frame"]);
}
