//! Request-handler handshake and dispatch tests plus their shared harness.
//!
//! Task lifecycle and authorization cases live in focused child modules so
//! each suite has room for future regressions without approaching the file cap.

mod authorization;
mod lifecycle;

use std::sync::Arc;

use serde_json::{json, Value};

use super::super::grants::{Grant, GrantRegistry};
use super::super::server::{handle_control, SessionState, TaskRegistry};
use super::super::types::{FleetOps, ToolFamilies, PROTOCOL_VERSION};
use super::{FakeFleet, FakeOutcome};

/// A handler under test: a fleet, a registry, a minted grant, and a connection.
pub(super) struct Harness {
    /// The fleet as the handler sees it.
    ops: Arc<dyn FleetOps>,
    /// The same fleet, typed, so a test can read what actually reached it.
    pub(super) fake: Arc<FakeFleet>,
    grants: GrantRegistry,
    registry: TaskRegistry,
    session: SessionState,
    token: String,
}

impl Harness {
    /// Build a harness over `fleet` whose grant is `grant`.
    pub(super) fn with(fleet: FakeFleet, grant: Grant) -> Self {
        let grants = GrantRegistry::new();
        let token = grants.mint(grant);
        let fake = Arc::new(fleet);
        Harness {
            ops: fake.clone(),
            fake,
            grants,
            registry: TaskRegistry::new(),
            session: SessionState::default(),
            token,
        }
    }

    /// The default harness: a connected fleet and a grant that may dispatch.
    pub(super) fn new() -> Self {
        Self::with(FakeFleet::new(), Grant::new("session-1", 0, 2))
    }

    /// Send one op without completing a handshake first.
    async fn raw(&mut self, op: &str, params: Value) -> Value {
        let request = json!({ "v": PROTOCOL_VERSION, "id": 1, "op": op, "params": params });
        handle_control(
            &self.ops,
            &self.grants,
            &self.registry,
            &mut self.session,
            &request,
        )
        .await
    }

    /// Complete the handshake, asserting it succeeded.
    async fn hello(&mut self) -> Value {
        let token = self.token.clone();
        let response = self
            .raw(
                "hello",
                json!({ "protocol": PROTOCOL_VERSION, "token": token }),
            )
            .await;
        assert_eq!(response["ok"], json!(true), "handshake failed: {response}");
        response["result"].clone()
    }

    /// Handshake, then send `op`.
    pub(super) async fn call(&mut self, op: &str, params: Value) -> Value {
        if self.session.grant().is_none() {
            self.hello().await;
        }
        self.raw(op, params).await
    }
}

/// The error slug on a refusal.
pub(super) fn kind(response: &Value) -> &str {
    response["error"]["kind"].as_str().unwrap_or("<none>")
}

#[tokio::test]
async fn a_handshake_reports_the_fleet_and_the_grant() {
    let result = Harness::new().hello().await;

    assert_eq!(result["protocol"], json!(PROTOCOL_VERSION));
    assert_eq!(result["hubReady"], json!(true));
    assert_eq!(result["depth"], json!(0));
    assert_eq!(result["maxDepth"], json!(2));
    assert_eq!(result["inFlight"], json!(0));
    assert_eq!(result["families"]["fleet"], json!(true));
}

#[tokio::test]
async fn a_child_grant_inherits_authority_at_the_next_depth() {
    let parent = Grant::new("nested", 1, 3)
        .with_families(ToolFamilies::workflows_only())
        .with_max_in_flight(2)
        .with_tool_mode(Some("propose:workflow-a"));
    let mut harness = Harness::with(FakeFleet::new(), parent.clone());

    let response = harness.call("grant.child", json!({})).await;
    let token = response["result"]["token"].as_str().unwrap();
    let child = harness.grants.redeem(token).expect("minted child grant");

    assert_eq!(response["result"]["depth"], json!(2));
    assert_eq!(child.session, parent.session);
    assert_eq!(child.depth, 2);
    assert_eq!(child.max_depth, parent.max_depth);
    assert_eq!(child.families, parent.families);
    assert_eq!(child.max_in_flight, parent.max_in_flight);
    assert_eq!(child.tool_mode, parent.tool_mode);
}

#[tokio::test]
async fn nothing_is_answered_before_a_handshake() {
    // The whole surface is scoped by grant, so an op that arrives without one
    // has no scope to be answered within — including a read as harmless-looking
    // as the roster.
    let mut harness = Harness::new();

    for op in ["worker.list", "task.list", "task.dispatch", "task.abort"] {
        let response = harness.raw(op, json!({})).await;
        assert_eq!(response["ok"], json!(false), "{op} was answered");
        assert_eq!(kind(&response), "unauthenticated", "{op}");
    }
}

#[tokio::test]
async fn an_unknown_token_is_refused() {
    let mut harness = Harness::new();

    let response = harness
        .raw(
            "hello",
            json!({ "protocol": PROTOCOL_VERSION, "token": "not-a-real-grant" }),
        )
        .await;

    assert_eq!(kind(&response), "unauthenticated");
    assert!(harness.session.grant().is_none());
}

#[tokio::test]
async fn revocation_stops_an_already_authenticated_connection() {
    let mut harness = Harness::new();
    harness.hello().await;

    harness.grants.revoke("session-1");
    let response = harness.raw("worker.list", json!({})).await;

    assert_eq!(kind(&response), "unauthenticated");
    assert!(harness.session.grant().is_none());
}

#[tokio::test]
async fn a_protocol_from_another_build_is_named_as_such() {
    let mut harness = Harness::new();
    let token = harness.token.clone();

    let response = harness
        .raw("hello", json!({ "protocol": 999, "token": token }))
        .await;

    assert_eq!(kind(&response), "versionMismatch");
}

#[tokio::test]
async fn an_unknown_op_is_a_bad_request() {
    let response = Harness::new().call("task.teleport", json!({})).await;

    assert_eq!(kind(&response), "badRequest");
}

#[tokio::test]
async fn an_operator_only_harness_cannot_be_dispatched() {
    let mut harness = Harness::new();
    let response = harness
        .call(
            "task.dispatch",
            json!({ "instruction": "do the thing", "harness": "openhuman" }),
        )
        .await;

    assert_eq!(kind(&response), "badRequest");
    assert!(harness.fake.dispatched.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_roster_read_returns_the_workers_and_the_default() {
    let response = Harness::new().call("worker.list", json!({})).await;

    let result = &response["result"];
    assert_eq!(result["workers"][0]["id"], json!("alpha"));
    assert_eq!(result["defaultWorker"], json!("alpha-address"));
}

#[tokio::test]
async fn a_connecting_hub_is_distinguishable_from_an_empty_fleet() {
    // The distinction this test exists for: a caller handed an empty list at
    // second zero concludes the fleet is unusable and stops asking, where
    // "still connecting" is an invitation to try again. Returning `[]` for both
    // is the bug.
    let mut harness = Harness::with(FakeFleet::unconnected(), Grant::new("s", 0, 2));

    let response = harness.call("worker.list", json!({})).await;

    assert_eq!(kind(&response), "hubNotReady");
    assert_eq!(response["error"]["retryable"], json!(true));

    let empty = Harness::with(FakeFleet::new().with_workers(vec![]), Grant::new("s", 0, 2))
        .call("worker.list", json!({}))
        .await;
    assert_eq!(empty["ok"], json!(true));
    assert_eq!(empty["result"]["workers"], json!([]));
}

#[tokio::test]
async fn a_dispatch_returns_a_handle_without_waiting() {
    let mut harness = Harness::new();

    let response = harness
        .call("task.dispatch", json!({ "instruction": "do the thing" }))
        .await;

    let result = &response["result"];
    assert_eq!(result["status"], json!("running"));
    assert_eq!(result["worker"], json!("alpha-address"));
    assert!(result["taskId"].as_str().unwrap().starts_with("mcp-"));
}

#[tokio::test]
async fn a_workflow_dispatch_carries_its_declared_inputs_to_the_runner() {
    let mut harness = Harness::new();
    let dispatched = harness
        .call(
            "task.dispatch",
            json!({
                "instruction": "release trigger",
                "workflow": "release",
                "workflowFingerprint": "release-fingerprint",
                "inputs": { "environment": "staging", "retries": 2 },
            }),
        )
        .await;
    let handle = dispatched["result"]["taskId"].as_str().unwrap().to_string();
    harness
        .call("task.get", json!({ "taskId": handle, "waitSeconds": 5 }))
        .await;

    let requests = harness.fake.dispatched.lock().unwrap();
    let request = requests.first().expect("the dispatch reached the fleet");
    assert_eq!(request.workflow.as_deref(), Some("release"));
    assert_eq!(
        request.workflow_fingerprint.as_deref(),
        Some("release-fingerprint")
    );
    assert_eq!(
        request.workflow_inputs,
        json!({ "environment": "staging", "retries": 2 })
            .as_object()
            .unwrap()
            .clone()
    );
}

#[tokio::test]
async fn workflow_inputs_must_be_an_object_and_name_a_workflow() {
    let wrong_shape = Harness::new()
        .call(
            "task.dispatch",
            json!({
                "instruction": "x",
                "workflow": "release",
                "workflowFingerprint": "release-fingerprint",
                "inputs": ["staging"]
            }),
        )
        .await;
    assert_eq!(kind(&wrong_shape), "badRequest");

    let no_workflow = Harness::new()
        .call(
            "task.dispatch",
            json!({ "instruction": "x", "inputs": { "environment": "staging" } }),
        )
        .await;
    assert_eq!(kind(&no_workflow), "badRequest");
}

#[tokio::test]
async fn a_workflow_dispatch_rejects_direct_harness_routing_hints() {
    for hint in [json!({ "harness": "claude" }), json!({ "model": "opus" })] {
        let mut params = json!({
            "instruction": "x",
            "workflow": "release",
            "workflowFingerprint": "release-fingerprint"
        });
        params
            .as_object_mut()
            .unwrap()
            .extend(hint.as_object().unwrap().clone());
        let response = Harness::new().call("task.dispatch", params).await;

        assert_eq!(kind(&response), "badRequest");
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("saved workflows"));
    }
}

#[tokio::test]
async fn a_dispatch_without_an_instruction_is_a_bad_request() {
    let response = Harness::new().call("task.dispatch", json!({})).await;

    assert_eq!(kind(&response), "badRequest");
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("instruction"));
}

#[tokio::test]
async fn an_unknown_worker_is_refused_with_the_candidates() {
    // Naming the alternatives turns a typo into a one-round-trip recovery
    // instead of forcing a roster read first.
    let response = Harness::new()
        .call(
            "task.dispatch",
            json!({ "instruction": "x", "worker": "beta" }),
        )
        .await;

    assert_eq!(kind(&response), "noSuchWorker");
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("alpha"));
}

#[tokio::test]
async fn an_unknown_harness_names_the_ones_that_exist() {
    let response = Harness::new()
        .call(
            "task.dispatch",
            json!({ "instruction": "x", "harness": "gpt" }),
        )
        .await;

    assert_eq!(kind(&response), "badRequest");
    let message = response["error"]["message"].as_str().unwrap();
    assert!(message.contains("claude") && message.contains("codex"));
}

#[tokio::test]
async fn medulla_mints_the_ids_and_keeps_the_task_context_free() {
    let mut harness = Harness::new();

    let dispatched = harness
        .call(
            "task.dispatch",
            json!({
                "instruction": "x",
                // A caller trying to choose its own handles. These are
                // capability keys on a registry shared by every dispatch on the
                // runner, so honouring them would let one holder cancel or
                // dedupe another's work.
                "taskId": "attacker-chosen",
                "abortId": "attacker-chosen",
                "conversation": "shared-context",
            }),
        )
        .await;
    let handle = dispatched["result"]["taskId"].as_str().unwrap().to_string();
    // Let the spawned dispatch reach the fake.
    harness
        .call("task.get", json!({ "taskId": handle, "waitSeconds": 5 }))
        .await;

    let requests = harness.fake.dispatched.lock().unwrap();
    let request = requests.first().expect("the dispatch reached the fleet");
    assert!(request.task_id.starts_with("mcp-"));
    assert!(request.abort_id.starts_with("mcp-"));
    assert_ne!(request.task_id, "attacker-chosen");
    assert_ne!(request.abort_id, "attacker-chosen");
    // The wire id and the abort key are distinct: one is the worker's dedupe
    // key, the other indexes a runner-wide abort registry.
    assert_ne!(request.task_id, request.abort_id);
    // Context-free unless asked, which is what lets two dispatches run
    // concurrently without seeing each other's work.
    assert_eq!(request.conversation, None);
    assert_eq!(request.tool_mode, None);
}

#[tokio::test]
async fn a_proposal_grant_keeps_delegated_work_in_proposal_mode() {
    let grant = Grant::new("review-session", 0, 2).with_tool_mode(Some("propose:workflow-a"));
    let mut harness = Harness::with(FakeFleet::new(), grant);

    let dispatched = harness
        .call("task.dispatch", json!({ "instruction": "review this" }))
        .await;
    let handle = dispatched["result"]["taskId"].as_str().unwrap();
    harness
        .call("task.get", json!({ "taskId": handle, "waitSeconds": 5 }))
        .await;

    let requests = harness.fake.dispatched.lock().unwrap();
    assert_eq!(
        requests
            .first()
            .and_then(|request| request.tool_mode.as_deref()),
        Some("propose:workflow-a")
    );
}

#[tokio::test]
async fn a_proposal_grant_cannot_run_a_saved_workflow_through_the_fleet() {
    let grant = Grant::new("review-session", 0, 2).with_tool_mode(Some("propose:workflow-a"));
    let mut harness = Harness::with(FakeFleet::new(), grant);

    let response = harness
        .call(
            "task.dispatch",
            json!({ "instruction": "run it", "workflow": "dangerous" }),
        )
        .await;

    assert_eq!(kind(&response), "badRequest");
    assert!(harness.fake.dispatched.lock().unwrap().is_empty());
}
