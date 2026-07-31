//! Durable prompt evidence captured from real agent-node execution.

use super::*;

#[tokio::test]
async fn an_agent_step_records_the_prompt_after_expression_resolution() {
    let harness = Harness::new();
    harness.install(
        &json!({
            "id": "resolved-prompt",
            "name": "Resolved prompt",
            "nodes": [
                { "id": "t", "kind": "trigger", "name": "start",
                  "config": { "trigger_kind": "manual" } },
                { "id": "work", "kind": "agent", "name": "Work",
                  "config": { "prompt": "=item.task", "agent_ref": "builder" } }
            ],
            "edges": [{ "from_node": "t", "to_node": "work" }]
        })
        .to_string(),
        "resolved-prompt",
    );
    let dispatch = Arc::new(StubDispatch::default());

    let record = run_workflow(
        harness.context(dispatch.clone()),
        "resolved-prompt",
        "run-resolved",
        json!({ "task": "Review the complete patch\nand report every risk." }),
        Default::default(),
    )
    .await
    .expect("runs");

    assert_eq!(
        dispatch.seen.lock().unwrap()[0].instruction,
        "Review the complete patch\nand report every risk."
    );
    assert_eq!(
        record.steps[0].input,
        Some(json!("Review the complete patch\nand report every risk."))
    );
}

#[tokio::test]
async fn a_workflow_s_defaults_block_reaches_the_dispatch() {
    // The layer between a node and the host: nothing in the graph says which
    // harness to use, so the document's own block is what a step must run on.
    let harness = Harness::new();
    harness.install(
        &json!({
            "id": "pinned",
            "name": "Pinned",
            "defaults": { "harness": "codex", "model": "gpt-5-codex" },
            "nodes": [
                { "id": "t", "kind": "trigger", "name": "start",
                  "config": { "trigger_kind": "manual" } },
                { "id": "work", "kind": "agent", "name": "Work",
                  "config": { "prompt": "go", "agent_ref": "builder" } }
            ],
            "edges": [{ "from_node": "t", "to_node": "work" }]
        })
        .to_string(),
        "pinned",
    );
    let dispatch = Arc::new(StubDispatch::default());

    run_workflow(
        harness.context(dispatch.clone()),
        "pinned",
        "run-pinned",
        json!({}),
        Default::default(),
    )
    .await
    .expect("runs");

    let seen = dispatch.seen.lock().unwrap();
    assert_eq!(
        seen[0].provider,
        Some(crate::tinyplace::HarnessProvider::Codex)
    );
    assert_eq!(seen[0].model.as_deref(), Some("gpt-5-codex"));
}

#[tokio::test]
async fn a_step_that_names_its_own_harness_overrides_the_workflow_s() {
    let harness = Harness::new();
    harness.install(
        &json!({
            "id": "mixed",
            "name": "Mixed",
            "defaults": { "harness": "codex", "model": "gpt-5-codex" },
            "nodes": [
                { "id": "t", "kind": "trigger", "name": "start",
                  "config": { "trigger_kind": "manual" } },
                { "id": "work", "kind": "agent", "name": "Work",
                  "config": { "prompt": "go", "agent_ref": "builder",
                              "harness": "claude" } }
            ],
            "edges": [{ "from_node": "t", "to_node": "work" }]
        })
        .to_string(),
        "mixed",
    );
    let dispatch = Arc::new(StubDispatch::default());

    run_workflow(
        harness.context(dispatch.clone()),
        "mixed",
        "run-mixed",
        json!({}),
        Default::default(),
    )
    .await
    .expect("runs");

    let seen = dispatch.seen.lock().unwrap();
    assert_eq!(
        seen[0].provider,
        Some(crate::tinyplace::HarnessProvider::Claude)
    );
    // The workflow's Codex model must not follow the step onto Claude Code.
    assert_eq!(seen[0].model, None);
}

#[tokio::test]
async fn a_persisted_harness_expression_is_refused_at_run_time() {
    // `FileWorkflowStore::save` only re-runs the engine's own structural
    // validation, not the authoring gates (`gates::check`, which the tool
    // surface's `workflow_create`/`apply_ops` run before a write lands) — so a
    // graph reaching the store by any other route, like an operator hand-
    // editing the saved JSON file, can still carry `harness` written as a
    // `=`-expression. `run_workflow` must catch it anyway: by the time a node
    // dispatches, the engine has already resolved the expression, and an
    // upstream node's own output would be choosing which binary and which
    // credentials run the step.
    let harness = Harness::new();
    harness.install(
        &json!({
            "id": "sneaky",
            "name": "Sneaky",
            "nodes": [
                { "id": "t", "kind": "trigger", "name": "start",
                  "config": { "trigger_kind": "manual" } },
                { "id": "pick", "kind": "transform", "name": "pick",
                  "config": { "set": { "harness": "=item.harness" } } },
                { "id": "work", "kind": "agent", "name": "Work",
                  "config": { "prompt": "go",
                              "harness": "=nodes.pick.item.json.harness" } }
            ],
            "edges": [
                { "from_node": "t", "to_node": "pick" },
                { "from_node": "pick", "to_node": "work" }
            ]
        })
        .to_string(),
        "sneaky",
    );
    let dispatch = Arc::new(StubDispatch::default());

    let err = run_workflow(
        harness.context(dispatch.clone()),
        "sneaky",
        "run-sneaky",
        json!({ "harness": "codex" }),
        Default::default(),
    )
    .await
    .expect_err("an unresolved harness expression must refuse the run");

    assert!(err.to_string().contains("expression"), "{err}");
    assert!(
        dispatch.seen.lock().unwrap().is_empty(),
        "a refused run must never reach the harness"
    );
}

#[tokio::test]
async fn a_sub_workflow_s_own_defaults_reach_its_agent_step() {
    // Workflow A names Claude; workflow B, which A invokes as a sub_workflow,
    // names Codex for itself. B's own step must run on Codex — a model chosen
    // for B is wrong handed to A's harness, and the reverse is just as wrong.
    let harness = Harness::new();
    harness.install(
        &json!({
            "id": "child",
            "name": "Child",
            "defaults": { "harness": "codex", "model": "gpt-5-codex" },
            "nodes": [
                { "id": "t", "kind": "trigger", "name": "start",
                  "config": { "trigger_kind": "manual" } },
                { "id": "work", "kind": "agent", "name": "Work",
                  "config": { "prompt": "child work" } }
            ],
            "edges": [{ "from_node": "t", "to_node": "work" }]
        })
        .to_string(),
        "child",
    );
    harness.install(
        &json!({
            "id": "parent",
            "name": "Parent",
            "defaults": { "harness": "claude", "model": "claude-sonnet" },
            "nodes": [
                { "id": "t", "kind": "trigger", "name": "start",
                  "config": { "trigger_kind": "manual" } },
                { "id": "sw", "kind": "sub_workflow", "name": "Invoke child",
                  "config": { "workflow_id": "child" } }
            ],
            "edges": [{ "from_node": "t", "to_node": "sw" }]
        })
        .to_string(),
        "parent",
    );
    let dispatch = Arc::new(StubDispatch::default());

    run_workflow(
        harness.context(dispatch.clone()),
        "parent",
        "run-parent",
        json!({}),
        Default::default(),
    )
    .await
    .expect("runs");

    let seen = dispatch.seen.lock().unwrap();
    assert_eq!(
        seen[0].provider,
        Some(crate::tinyplace::HarnessProvider::Codex),
        "the child's own defaults, not the parent's, must reach its step"
    );
    assert_eq!(seen[0].model.as_deref(), Some("gpt-5-codex"));
}
