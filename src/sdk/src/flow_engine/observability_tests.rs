//! Tests for the run observer.
//!
//! The load-bearing claim is the last test: what the observer emits folds
//! through the *existing* work pipeline and renders, so a workflow needs no
//! rendering code of its own.

use std::sync::Arc;

use serde_json::json;
use tinyflows::model::WorkflowGraph;
use tinyflows::observability::{ExecutionStep, Run, RunObserver, RunStatus, StepStatus};

use super::{folding_sink, recording_sink, WorkflowRunObserver};
use crate::harness_work::{kinds, WorkItemStatus};

/// A three-node graph: trigger, an agent step, then a transform.
fn graph() -> WorkflowGraph {
    serde_json::from_value(json!({
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start" },
            { "id": "build", "kind": "agent", "name": "Build the thing" },
            { "id": "report", "kind": "transform", "name": "Summarise" }
        ],
        "edges": [
            { "from_node": "t", "to_node": "build" },
            { "from_node": "build", "to_node": "report" }
        ]
    }))
    .unwrap()
}

fn step(node_id: &str, status: StepStatus) -> ExecutionStep {
    ExecutionStep {
        node_id: node_id.to_string(),
        status,
        output: json!({}),
        duration_ms: 5,
        diagnostics: Vec::new(),
        transcript: Vec::new(),
    }
}

#[test]
fn the_plan_lists_every_node_except_the_trigger() {
    let (sink, seen) = recording_sink();
    let observer = WorkflowRunObserver::new("demo", &graph(), sink);

    observer.on_run_start("run-1");

    let seen = seen.lock().unwrap();
    let plan = &seen[kinds::PLAN_UPDATE][0];
    let steps = plan["steps"].as_array().unwrap();

    assert_eq!(steps.len(), 2, "the trigger is not work to be done");
    assert_eq!(steps[0]["content"], "Build the thing");
    assert_eq!(steps[1]["content"], "Summarise");
    assert!(
        plan["goal"].as_str().unwrap().contains("run-1"),
        "the goal should name the run: {plan}"
    );
}

#[test]
fn a_finished_step_completes_and_the_next_one_becomes_active() {
    let (sink, seen) = recording_sink();
    let observer = WorkflowRunObserver::new("demo", &graph(), sink);

    observer.on_step_finish(&step("build", StepStatus::Success));

    let seen = seen.lock().unwrap();
    let todos = &seen[kinds::TODO_UPDATE][0]["todos"];
    assert_eq!(todos[0]["status"], "completed");
    assert_eq!(
        todos[1]["status"], "in_progress",
        "with no start callback, the next pending node is reported as active"
    );
}

#[test]
fn a_failed_step_is_marked_cancelled_rather_than_silently_completed() {
    let (sink, seen) = recording_sink();
    let observer = WorkflowRunObserver::new("demo", &graph(), sink);

    observer.on_step_finish(&step("build", StepStatus::Error));

    let seen = seen.lock().unwrap();
    assert_eq!(
        seen[kinds::TODO_UPDATE][0]["todos"][0]["status"],
        "cancelled"
    );
}

#[test]
fn an_agent_node_is_also_reported_as_a_subagent() {
    let (sink, seen) = recording_sink();
    let observer = WorkflowRunObserver::new("demo", &graph(), sink);

    observer.on_step_finish(&step("build", StepStatus::Success));
    observer.on_step_finish(&step("report", StepStatus::Success));

    let seen = seen.lock().unwrap();
    let subagents = &seen[kinds::SUBAGENT_START];
    assert_eq!(
        subagents.len(),
        1,
        "only the agent node dispatches to a harness"
    );
    assert_eq!(subagents[0]["description"], "Build the thing");
}

#[test]
fn steps_are_recorded_with_their_diagnostics_for_the_run_record() {
    let (sink, _) = recording_sink();
    let observer = WorkflowRunObserver::new("demo", &graph(), sink);
    let mut failing = step("build", StepStatus::Success);
    failing.output = json!({ "artifact": "report.md" });
    failing.diagnostics = vec![tinyflows::expr::NullResolution {
        location: "args.to".into(),
        expression: "=nodes.missing.items[0]".into(),
    }];

    observer.on_step_finish(&failing);

    let steps = observer.steps();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].node_id, "build");
    assert_eq!(steps[0].status, "success");
    assert_eq!(
        steps[0].output,
        Some(json!({ "artifact": "report.md" })),
        "the run detail needs the step's actual result"
    );
    assert!(
        steps[0].diagnostics[0].contains("args.to"),
        "an unresolved binding should name its location: {:?}",
        steps[0].diagnostics
    );
}

#[test]
fn the_result_says_which_node_failed() {
    let (sink, seen) = recording_sink();
    let observer = WorkflowRunObserver::new("demo", &graph(), sink);

    observer.on_run_finish(&Run {
        id: "run-1".into(),
        status: RunStatus::Failed,
        steps: vec![
            step("build", StepStatus::Success),
            step("report", StepStatus::Error),
        ],
    });

    let seen = seen.lock().unwrap();
    let result = &seen[kinds::RUN_RESULT][0];
    assert_eq!(result["ok"], false);
    assert_eq!(result["duration_ms"], 10, "durations sum across steps");
    assert!(
        result["summary"].as_str().unwrap().contains("report"),
        "name the node that failed: {result}"
    );
}

#[test]
fn a_run_folds_into_a_snapshot_the_existing_work_pane_renders() {
    // The whole point of speaking the harness_work vocabulary: no workflow
    // rendering code exists, and none is needed.
    let (sink, fold) = folding_sink();
    let observer = WorkflowRunObserver::new("demo", &graph(), sink);

    observer.on_run_start("run-1");
    observer.on_step_finish(&step("build", StepStatus::Success));
    observer.on_run_finish(&Run {
        id: "run-1".into(),
        status: RunStatus::Completed,
        steps: vec![step("build", StepStatus::Success)],
    });

    let fold = fold.lock().unwrap();
    let snapshot = fold.snapshot();

    assert_eq!(snapshot.todo_progress(), (1, 2), "one of two steps done");
    assert_eq!(
        snapshot.current_todo().map(|t| t.text.as_str()),
        Some("Summarise"),
        "the active step is the one an operator is waiting on"
    );
    assert_eq!(snapshot.plan.len(), 2);
    assert_eq!(snapshot.todos[0].status, WorkItemStatus::Completed);

    let lines = crate::ui::work::work_lines(snapshot, 80);
    assert!(
        !lines.is_empty(),
        "a workflow run must render through the existing pane"
    );
    let rendered = lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Build the thing"),
        "the plan should be visible: {rendered}"
    );
}

/// Keeps the sink type honest: it must be shareable across threads, since the
/// engine clones the observer into every node handler.
#[test]
fn the_observer_is_shareable_across_threads() {
    let (sink, _) = recording_sink();
    let observer: Arc<dyn RunObserver> = Arc::new(WorkflowRunObserver::new("demo", &graph(), sink));
    let moved = observer.clone();
    std::thread::spawn(move || moved.on_run_start("run-1"))
        .join()
        .unwrap();
}

#[test]
fn a_finished_agent_node_does_not_stay_open_as_a_running_subagent() {
    // The engine only tells us about an agent node once it has finished, so a
    // lone start would leave the terminal snapshot showing work in flight after
    // the run ended.
    let (sink, fold) = folding_sink();
    let observer = WorkflowRunObserver::new("demo", &graph(), sink);

    observer.on_step_finish(&step("build", StepStatus::Success));

    let fold = fold.lock().unwrap();
    let snapshot = fold.snapshot();
    assert_eq!(snapshot.subagents.len(), 1);
    assert_eq!(
        snapshot.running_subagents(),
        0,
        "the node has already finished: {:?}",
        snapshot.subagents
    );
}

#[test]
fn a_failed_agent_node_settles_as_failed_rather_than_done() {
    let (sink, fold) = folding_sink();
    let observer = WorkflowRunObserver::new("demo", &graph(), sink);

    observer.on_step_finish(&step("build", StepStatus::Error));

    let fold = fold.lock().unwrap();
    let subagent = &fold.snapshot().subagents[0];
    assert_eq!(
        subagent.status,
        crate::harness_work::SubagentStatus::Failed,
        "{subagent:?}"
    );
}
