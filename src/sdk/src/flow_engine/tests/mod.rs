//! Tests for the capability seam.
//!
//! Every test is offline and deterministic. The one that matters most drives a
//! real graph through the real engine into the real agent adapter, and asserts
//! on the task frame that came out the other side — that is the claim "a
//! workflow step is a harness session" reduced to something checkable.
//!
//! The `medulla:shell` tool's own cases live in the `shell_tests` submodule,
//! harness/model selection in `harness_selection_tests`, and tool-invoker cases
//! in `tools_tests`, each split out once they pushed this file over the
//! 500-line ceiling. The fixtures these modules share — `settings`,
//! `RecordingDispatch`, `empty_resolver`, `agent_graph` — are `pub(super)` for
//! that reason.
//!
//! What is *not* here any more: the state store's and the HTTP capsule's own
//! cases. Both implementations moved to `tinyflows::caps::host`, and their tests
//! went with them — testing them from here would be this crate asserting on
//! another crate's internals. `dry_run_tests` keeps the part that is still about
//! Medulla: that a simulated run starts no harness session.

mod dry_run_tests;
mod harness_selection_tests;
mod shell_tests;
mod tools_tests;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};
use tinyflows::model::WorkflowGraph;

use super::caps::agent::{instruction_of, reply_to_value, route_for_agent_ref, AgentRoute};
use super::caps::dispatch::HarnessDispatch;
use super::caps::{build_capabilities, HostServices};
use super::settings::CapabilitySettings;
use crate::hub::{RunError, TaskOutcome, TaskRequest};
use crate::workflows::StoreWorkflowResolver;

/// A dispatch that records what it was asked to run and replies with a fixed
/// answer, so a test can assert on the frame rather than on a live worker.
#[derive(Default)]
pub(super) struct RecordingDispatch {
    seen: Mutex<Vec<TaskRequest>>,
    reply: String,
    fail: Option<RunError>,
}

impl RecordingDispatch {
    /// A dispatch that answers every `TaskRequest` with `reply`.
    pub(super) fn replying(reply: &str) -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Vec::new()),
            reply: reply.to_string(),
            fail: None,
        })
    }

    fn failing(err: RunError) -> Arc<Self> {
        Arc::new(Self {
            seen: Mutex::new(Vec::new()),
            reply: String::new(),
            fail: Some(err),
        })
    }

    /// The requests this dispatch has received so far, in arrival order.
    pub(super) fn requests(&self) -> Vec<TaskRequest> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait]
impl HarnessDispatch for RecordingDispatch {
    async fn dispatch(&self, request: TaskRequest) -> Result<TaskOutcome, RunError> {
        self.seen.lock().unwrap().push(request);
        match &self.fail {
            Some(err) => Err(err.clone()),
            None => Ok(TaskOutcome {
                reply: self.reply.clone(),
                usage: crate::protocol::TokenUsage {
                    input_tokens: 0,
                    output_tokens: 0,
                },
                harness: None,
                session_id: None,
                transcript: Vec::new(),
            }),
        }
    }
}

/// A resolver over an empty store: no test here uses `sub_workflow`, but the
/// engine requires the capability to be present.
pub(super) fn empty_resolver(root: &std::path::Path) -> Arc<StoreWorkflowResolver> {
    Arc::new(StoreWorkflowResolver::new(
        Arc::new(crate::workflows::FileWorkflowStore::new(
            vec![root.join("workflows")],
            root.join("runs"),
        )),
        u64::MAX,
    ))
}

/// A capability settings tree rooted at `root`, with the default worker
/// address pinned to the local test worker.
pub(super) fn settings(root: &std::path::Path) -> Arc<CapabilitySettings> {
    let mut settings = CapabilitySettings::rooted_at(root);
    settings.default_worker_address = "local-worker".into();
    Arc::new(settings)
}

/// A one-agent-node graph: trigger into an agent, which is the smallest thing
/// that exercises the dispatch path end to end.
pub(super) fn agent_graph(config: Value) -> WorkflowGraph {
    serde_json::from_value(json!({
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start",
              "config": { "trigger_kind": "manual" } },
            { "id": "step", "kind": "agent", "name": "step", "config": config }
        ],
        "edges": [{ "from_node": "t", "to_node": "step" }]
    }))
    .unwrap()
}

#[tokio::test]
async fn an_agent_node_becomes_a_task_frame_dispatched_to_a_harness() {
    let root = tempfile::tempdir().unwrap();
    let dispatch = RecordingDispatch::replying("done: refactored the parser");
    let caps = build_capabilities(
        settings(root.path()),
        HostServices {
            node_progress: None,
            dispatch: dispatch.clone(),
            resolver: empty_resolver(root.path()),
            http_credentials: HashMap::new(),
        },
        "workflow:demo",
        "run-7",
    );

    let compiled = tinyflows::compiler::compile(&agent_graph(json!({
        "prompt": "refactor the parser",
        "agent_ref": "builder"
    })))
    .expect("compiles");
    let outcome = tinyflows::engine::run(&compiled, json!({}), &caps)
        .await
        .expect("runs");

    let requests = dispatch.requests();
    assert_eq!(requests.len(), 1, "one node, one dispatch");
    let request = &requests[0];
    assert_eq!(request.instruction, "refactor the parser");
    assert_eq!(
        request.worker_address, "builder",
        "the agent_ref selects the worker"
    );
    assert_eq!(
        request.abort_id, "run-7",
        "aborting the run must cancel the node in flight"
    );

    // The reply reaches the node's output through the engine's envelope, which
    // is what makes `=item.text` resolve for a downstream node.
    assert_eq!(
        outcome.output["nodes"]["step"]["items"][0]["json"]["text"],
        "done: refactored the parser"
    );
}

#[tokio::test]
async fn a_node_without_an_agent_ref_runs_on_the_configured_default_worker() {
    let root = tempfile::tempdir().unwrap();
    let dispatch = RecordingDispatch::replying("ok");
    let caps = build_capabilities(
        settings(root.path()),
        HostServices {
            node_progress: None,
            dispatch: dispatch.clone(),
            resolver: empty_resolver(root.path()),
            http_credentials: HashMap::new(),
        },
        "workflow:demo",
        "run-1",
    );

    let compiled =
        tinyflows::compiler::compile(&agent_graph(json!({ "prompt": "hello" }))).unwrap();
    tinyflows::engine::run(&compiled, json!({}), &caps)
        .await
        .expect("runs");

    assert_eq!(dispatch.requests()[0].worker_address, "local-worker");
}

#[tokio::test]
async fn a_worker_failure_surfaces_as_a_capability_error_naming_the_cause() {
    let root = tempfile::tempdir().unwrap();
    let dispatch = RecordingDispatch::failing(RunError::Worker("clone failed".into()));
    let caps = build_capabilities(
        settings(root.path()),
        HostServices {
            node_progress: None,
            dispatch,
            resolver: empty_resolver(root.path()),
            http_credentials: HashMap::new(),
        },
        "workflow:demo",
        "run-2",
    );

    let compiled =
        tinyflows::compiler::compile(&agent_graph(json!({ "prompt": "build" }))).unwrap();
    let err = tinyflows::engine::run(&compiled, json!({}), &caps)
        .await
        .expect_err("the node must fail");

    assert!(
        err.to_string().contains("clone failed"),
        "the worker's own message should survive: {err}"
    );
}

#[tokio::test]
async fn dispatching_with_no_worker_configured_explains_what_to_set() {
    let root = tempfile::tempdir().unwrap();
    let mut bare = CapabilitySettings::rooted_at(root.path());
    bare.default_worker_address = String::new();
    let caps = build_capabilities(
        Arc::new(bare),
        HostServices {
            node_progress: None,
            dispatch: RecordingDispatch::replying("unused"),
            resolver: empty_resolver(root.path()),
            http_credentials: HashMap::new(),
        },
        "workflow:demo",
        "run-3",
    );

    let compiled = tinyflows::compiler::compile(&agent_graph(json!({ "prompt": "hi" }))).unwrap();
    let err = tinyflows::engine::run(&compiled, json!({}), &caps)
        .await
        .expect_err("nowhere to send it");

    assert!(
        err.to_string().contains("agent_ref"),
        "the message should say how to fix it: {err}"
    );
}

#[test]
fn an_empty_agent_ref_routes_to_the_default_rather_than_a_worker_named_nothing() {
    assert_eq!(route_for_agent_ref(None), AgentRoute::Default);
    assert_eq!(route_for_agent_ref(Some("   ")), AgentRoute::Default);
    assert_eq!(
        route_for_agent_ref(Some("builder")),
        AgentRoute::Template("builder".into())
    );
}

#[test]
fn a_node_may_name_its_instruction_prompt_or_instruction() {
    assert_eq!(
        instruction_of(&json!({ "prompt": "a" })).unwrap(),
        "a".to_string()
    );
    assert_eq!(
        instruction_of(&json!({ "instruction": "b" })).unwrap(),
        "b".to_string()
    );
    let err = instruction_of(&json!({ "prompt": "  " })).expect_err("blank is not an instruction");
    assert!(err.to_string().contains("prompt"), "got {err}");
}

#[test]
fn production_capabilities_gate_shell_execution_on_the_same_switch_as_code() {
    let root = tempfile::tempdir().unwrap();

    // `allow_code` on: a `shell` node runs with this host's policy, same as
    // `build_capabilities_inner`'s comment promises ("offered exactly when a
    // `code` node is").
    let mut allowed = settings(root.path());
    Arc::get_mut(&mut allowed).unwrap().allow_code = true;
    let caps = build_capabilities(
        allowed,
        HostServices {
            node_progress: None,
            dispatch: RecordingDispatch::replying("unused"),
            resolver: empty_resolver(root.path()),
            http_credentials: HashMap::new(),
        },
        "workflow:demo",
        "run-shell-boundary",
    );
    assert!(
        caps.shell.is_some(),
        "shell nodes must be available once this host's script policy (path, environment, interpreter) is wired up"
    );

    // `allow_code` off: refused, exactly like `code` nodes are.
    let mut denied = settings(root.path());
    Arc::get_mut(&mut denied).unwrap().allow_code = false;
    let caps = build_capabilities(
        denied,
        HostServices {
            node_progress: None,
            dispatch: RecordingDispatch::replying("unused"),
            resolver: empty_resolver(root.path()),
            http_credentials: HashMap::new(),
        },
        "workflow:demo",
        "run-shell-boundary-denied",
    );
    assert!(
        caps.shell.is_none(),
        "an operator who disabled code execution must not get shell execution through the back door"
    );
}

#[test]
fn a_json_reply_is_surfaced_structurally_as_well_as_textually() {
    let value = reply_to_value("{\"files\": 3}", "builder");
    assert_eq!(value["json"]["files"], 3);
    assert_eq!(value["text"], "{\"files\": 3}");

    // Prose is still fine; it just has no structure to offer.
    let prose = reply_to_value("all done", "builder");
    assert!(prose["json"].is_null());
    assert_eq!(prose["text"], "all done");
}

#[tokio::test]
async fn two_nodes_on_one_worker_get_distinct_wire_ids() {
    // A worker dedupes on sender + taskId, so two parallel nodes sharing an id
    // would have one silently rejected as a duplicate of the other.
    let root = tempfile::tempdir().unwrap();
    let dispatch = RecordingDispatch::replying("ok");
    let caps = build_capabilities(
        settings(root.path()),
        HostServices {
            node_progress: None,
            dispatch: dispatch.clone(),
            resolver: empty_resolver(root.path()),
            http_credentials: HashMap::new(),
        },
        "workflow:demo",
        "run-dup",
    );

    let graph: WorkflowGraph = serde_json::from_value(json!({
        "nodes": [
            { "id": "t", "kind": "trigger", "name": "start" },
            { "id": "left", "kind": "agent", "name": "L", "config": { "prompt": "a" } },
            { "id": "right", "kind": "agent", "name": "R", "config": { "prompt": "b" } }
        ],
        "edges": [
            { "from_node": "t", "to_node": "left" },
            { "from_node": "t", "to_node": "right" }
        ]
    }))
    .unwrap();
    let compiled = tinyflows::compiler::compile(&graph).unwrap();
    tinyflows::engine::run(&compiled, json!({}), &caps)
        .await
        .expect("runs");

    let ids: Vec<String> = dispatch
        .requests()
        .iter()
        .map(|r| r.task_id.clone())
        .collect();
    assert_eq!(ids.len(), 2);
    assert_ne!(
        ids[0], ids[1],
        "both nodes routed to the default worker: {ids:?}"
    );
}
