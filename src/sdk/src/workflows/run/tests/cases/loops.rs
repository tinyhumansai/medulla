//! Loop nodes: iteration counts, host clamping, and declared caps.
//!
//! Split from the sibling case module to keep each under the repository's
//! 500-line ceiling; they share its harness through `use super::*`.

use super::*;

/// A bounded loop whose body is a single agent node, closed by a back-edge.
fn bounded_loop(max_iterations: u64) -> String {
    json!({
        "name": "Bounded loop",
        "description": "Repeats one agent step a bounded number of times.",
        "nodes": [
            { "id": "start", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "l", "kind": "loop", "name": "Until done",
              "config": { "max_iterations": max_iterations, "on_exceeded": "continue" } },
            { "id": "work", "kind": "agent", "name": "Work",
              "config": { "prompt": "Do one pass of the work." } },
            { "id": "out", "kind": "transform", "name": "Report",
              "config": { "set": { "passes": "=.nodes.l.iteration" } } }
        ],
        "edges": [
            { "from_node": "start", "to_node": "l" },
            { "from_node": "l", "from_port": "body", "to_node": "work" },
            { "from_node": "work", "to_node": "l" },
            { "from_node": "l", "from_port": "done", "to_node": "out" }
        ]
    })
    .to_string()
}

#[tokio::test]
async fn a_bounded_loop_dispatches_its_body_once_per_iteration() {
    let harness = Harness::new();
    harness.install(&bounded_loop(3), "loop");
    let dispatch = Arc::new(StubDispatch::default());

    let record = run_workflow(
        harness.context(dispatch.clone()),
        "loop",
        "run-loop-1",
        json!({}),
        Default::default(),
    )
    .await
    .expect("runs");

    assert_eq!(record.status, RunStatus::Succeeded);
    assert_eq!(
        dispatch.seen.lock().unwrap().len(),
        3,
        "the agent node in the body should run once per iteration"
    );
}

/// The host ceiling is a clamp, not a refusal: a graph asking for more than the
/// operator allows still runs, it just stops sooner. Refusing would make a
/// workflow authored against a more generous host unrunnable here.
#[tokio::test]
async fn a_loop_asking_past_the_host_ceiling_is_clamped_rather_than_refused() {
    let mut harness = Harness::new();
    let mut settings = CapabilitySettings::clone(&harness.settings);
    settings.max_loop_iterations = 2;
    harness.settings = Arc::new(settings);

    harness.install(&bounded_loop(50), "greedy");
    let dispatch = Arc::new(StubDispatch::default());

    let record = run_workflow(
        harness.context(dispatch.clone()),
        "greedy",
        "run-loop-2",
        json!({}),
        Default::default(),
    )
    .await
    .expect("a clamped loop still runs");

    assert_eq!(record.status, RunStatus::Succeeded);
    assert_eq!(
        dispatch.seen.lock().unwrap().len(),
        2,
        "the loop should stop at the host ceiling, not at the 50 it asked for"
    );

    // The stored document keeps what the author wrote, so raising the ceiling
    // later restores the intent without anyone re-editing the workflow.
    let saved = harness.store.get("greedy").expect("gets").expect("present");
    let declared = saved
        .graph
        .nodes
        .iter()
        .find(|n| n.id == "l")
        .and_then(|n| n.config.get("max_iterations"))
        .and_then(|v| v.as_u64());
    assert_eq!(
        declared,
        Some(50),
        "the clamp must not rewrite the document"
    );
}

/// A loop that omits `max_iterations` falls through to the engine's own
/// default (25) — not to "no limit". A host ceiling below that default must
/// still bind it, or a graph that names no cap at all can outrun a ceiling an
/// operator lowered specifically to bound cost.
#[tokio::test]
async fn a_loop_with_no_declared_cap_is_still_clamped_to_the_host_ceiling() {
    let mut harness = Harness::new();
    let mut settings = CapabilitySettings::clone(&harness.settings);
    settings.max_loop_iterations = 2;
    harness.settings = Arc::new(settings);

    let document = json!({
        "name": "Uncapped loop",
        "nodes": [
            { "id": "start", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "l", "kind": "loop", "name": "Until done",
              "config": { "on_exceeded": "continue" } },
            { "id": "work", "kind": "agent", "name": "Work",
              "config": { "prompt": "Do one pass of the work." } },
            { "id": "out", "kind": "transform", "name": "Report",
              "config": { "set": { "passes": "=.nodes.l.iteration" } } }
        ],
        "edges": [
            { "from_node": "start", "to_node": "l" },
            { "from_node": "l", "from_port": "body", "to_node": "work" },
            { "from_node": "work", "to_node": "l" },
            { "from_node": "l", "from_port": "done", "to_node": "out" }
        ]
    })
    .to_string();
    harness.install(&document, "uncapped");
    let dispatch = Arc::new(StubDispatch::default());

    let record = run_workflow(
        harness.context(dispatch.clone()),
        "uncapped",
        "run-loop-uncapped",
        json!({}),
        Default::default(),
    )
    .await
    .expect("a clamped loop still runs");

    assert_eq!(record.status, RunStatus::Succeeded);
    assert_eq!(
        dispatch.seen.lock().unwrap().len(),
        2,
        "an implicit engine default (25) must still be clamped to the host ceiling"
    );
}
