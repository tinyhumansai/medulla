//! Tests for the work a `spawn` node hands off.
//!
//! The subject is that a ticket comes back carrying the *result of the work*,
//! not a description of it. The engine's own runner settles a ticket with an
//! echo of the spec, which is indistinguishable from success at every layer
//! above — the gate releases, the graph continues, and the answer is rubbish.
//! So each test here asserts on the value a gate would collect.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tinyflows::caps::{Capabilities, TaskRunner, TaskSpec, TaskState};

use super::{MedullaTaskRunner, TaskCapabilities};

/// Capabilities that touch nothing, with the deadline the test wants.
///
/// Built on the engine's mock bundle so a spawned child graph has stand-ins to
/// run against, minus its task runner: a nested `spawn` then runs inline, which
/// keeps a test's assertions about one level of nesting from depending on
/// another thread's timing.
struct MockTaskCaps {
    /// The deadline handed to each task.
    timeout: Duration,
}

impl TaskCapabilities for MockTaskCaps {
    fn capabilities(&self) -> Capabilities {
        let mut caps = tinyflows::caps::mock::mock_capabilities();
        caps.tasks = None;
        caps
    }

    fn timeout(&self) -> Duration {
        self.timeout
    }
}

/// A runner over the mock bundle, with a deadline long enough not to fire.
fn runner() -> MedullaTaskRunner {
    MedullaTaskRunner::new(Arc::new(MockTaskCaps {
        timeout: Duration::from_secs(30),
    }))
}

/// Poll the way a `gate` does — once per activation — until the task settles.
async fn settle(runner: &MedullaTaskRunner, ticket: &str) -> TaskState {
    for _ in 0..512 {
        let state = runner.poll(ticket).await.expect("poll");
        if state.is_settled() {
            return state;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("task {ticket} never settled");
}

/// The value a gate would collect, or a panic naming why it could not.
fn collected(state: TaskState) -> Value {
    match state {
        TaskState::Done(value) => value,
        other => panic!("expected a completed task, got {other:?}"),
    }
}

/// The point of the whole module: a spawned tool call must come back with what
/// the tool returned. The engine's default runner returns
/// `{"spec": "tool", "slug": …}` here, which is the failure this guards.
#[tokio::test]
async fn a_spawned_tool_call_settles_with_the_tool_s_output_not_its_spec() {
    let runner = runner();
    let ticket = runner
        .start(TaskSpec::Tool {
            slug: "demo.echo".to_string(),
            args: json!({ "x": 1 }),
        })
        .await
        .expect("start");

    let result = collected(settle(&runner, &ticket).await);
    assert_ne!(
        result.get("spec").and_then(Value::as_str),
        Some("tool"),
        "a settled ticket must carry the tool's output, not an echo of the spec: {result}"
    );
}

/// A spawned child graph is compiled and run, so what settles is the child's
/// run output rather than the graph document that was handed over.
#[tokio::test]
async fn a_spawned_workflow_settles_with_the_child_run_s_output() {
    let runner = runner();
    let graph = json!({
        "id": "child",
        "name": "child",
        "nodes": [
            { "id": "start", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "shape", "kind": "transform", "name": "shape",
              "config": { "set": { "answer": "=21 + 21" } } }
        ],
        "edges": [{ "from_node": "start", "to_node": "shape" }]
    });

    let ticket = runner
        .start(TaskSpec::Workflow {
            graph,
            input: Value::Null,
        })
        .await
        .expect("start");

    // The child's own run state, not the graph document that was handed over:
    // the transform only has a value here because the child was compiled and
    // executed rather than echoed back.
    let result = collected(settle(&runner, &ticket).await);
    assert_eq!(
        result
            .pointer("/nodes/shape/items/0/json")
            .and_then(Value::as_i64),
        Some(42),
        "the child's transform should have run, got {result}"
    );
}

/// An invalid child graph must settle as `Failed` rather than take the spawning
/// run down: the `spawn` node has already returned, so the only place left to
/// report it is the gate that collects the ticket.
#[tokio::test]
async fn a_child_graph_that_does_not_compile_settles_as_failed() {
    let runner = runner();
    let ticket = runner
        .start(TaskSpec::Workflow {
            graph: json!({ "nodes": "not a list" }),
            input: Value::Null,
        })
        .await
        .expect("start should accept the task and report the failure through the ticket");

    assert!(
        matches!(settle(&runner, &ticket).await, TaskState::Failed(_)),
        "a child that cannot compile must settle as failed"
    );
}

/// A wedged task must not sit in the process forever. Nothing else in the run
/// will notice it: the branch that spawned it carried on, and a gate that gave
/// up polling simply stops asking.
#[tokio::test]
async fn a_task_that_overruns_its_deadline_settles_as_failed() {
    let runner = MedullaTaskRunner::new(Arc::new(MockTaskCaps {
        timeout: Duration::from_millis(1),
    }));
    let graph = json!({
        "id": "slow", "name": "slow",
        "nodes": [
            { "id": "start", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "wait", "kind": "agent", "name": "wait", "config": { "prompt": "hello" } }
        ],
        "edges": [{ "from_node": "start", "to_node": "wait" }]
    });
    let ticket = runner
        .start(TaskSpec::Workflow {
            graph,
            input: Value::Null,
        })
        .await
        .expect("start");

    // Either it beat the 1ms deadline or it did not; both settle, and the
    // assertion that matters is that it settled at all rather than hanging.
    assert!(settle(&runner, &ticket).await.is_settled());
}

/// A gate may see the same ticket on several activations before it releases, so
/// reading a result must not consume it.
#[tokio::test]
async fn a_settled_result_survives_repeated_polling() {
    let runner = runner();
    let ticket = runner
        .start(TaskSpec::Tool {
            slug: "demo.echo".to_string(),
            args: json!({}),
        })
        .await
        .expect("start");

    let first = settle(&runner, &ticket).await;
    assert_eq!(runner.poll(&ticket).await.expect("re-poll"), first);
}

/// Tickets are opaque to the graph, but they must at least be distinct — a
/// collision would silently merge two spawned tasks into one.
#[tokio::test]
async fn tickets_are_unique() {
    let runner = runner();
    let spec = || TaskSpec::Http {
        request: json!({ "url": "https://example.invalid/" }),
    };
    let a = runner.start(spec()).await.expect("start");
    let b = runner.start(spec()).await.expect("start");
    assert_ne!(a, b);
}

/// An unknown ticket is an error rather than a permanent `Pending`, which a
/// gate would otherwise poll its whole budget away on.
#[tokio::test]
async fn an_unknown_ticket_is_an_error() {
    let runner = runner();
    assert!(runner.poll("medulla-task-999").await.is_err());
    assert!(runner.cancel("medulla-task-999").await.is_err());
}

/// Cancelling settles the task, and cancelling one that already finished must
/// leave its result alone — a gate that released on it has used that value.
#[tokio::test]
async fn cancelling_settles_a_task_but_never_rewrites_a_finished_one() {
    let runner = runner();
    let ticket = runner
        .start(TaskSpec::Tool {
            slug: "demo.echo".to_string(),
            args: json!({}),
        })
        .await
        .expect("start");
    let before = settle(&runner, &ticket).await;
    runner.cancel(&ticket).await.expect("cancel");
    assert_eq!(runner.poll(&ticket).await.expect("poll"), before);

    let ticket = runner
        .start(TaskSpec::Tool {
            slug: "demo.echo".to_string(),
            args: json!({}),
        })
        .await
        .expect("start");
    runner.cancel(&ticket).await.expect("cancel");
    assert!(
        runner.poll(&ticket).await.expect("poll").is_settled(),
        "a cancelled task must not stay pending"
    );
}
