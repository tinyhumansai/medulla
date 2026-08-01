//! Request-handler branches: the handshake, each op's happy path, and every
//! refusal a caller has to be able to tell apart.

use std::sync::Arc;

use serde_json::{json, Value};

use super::super::grants::{Grant, GrantRegistry};
use super::super::server::{handle_control, SessionState, TaskRegistry};
use super::super::types::{FleetOps, ToolFamilies, PROTOCOL_VERSION};
use super::{FakeFleet, FakeOutcome};

/// A handler under test: a fleet, a registry, a minted grant, and a connection.
struct Harness {
    /// The fleet as the handler sees it.
    ops: Arc<dyn FleetOps>,
    /// The same fleet, typed, so a test can read what actually reached it.
    fake: Arc<FakeFleet>,
    grants: GrantRegistry,
    registry: TaskRegistry,
    session: SessionState,
    token: String,
}

impl Harness {
    /// Build a harness over `fleet` whose grant is `grant`.
    fn with(fleet: FakeFleet, grant: Grant) -> Self {
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
    fn new() -> Self {
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
    async fn call(&mut self, op: &str, params: Value) -> Value {
        if self.session.grant().is_none() {
            self.hello().await;
        }
        self.raw(op, params).await
    }
}

/// The error slug on a refusal.
fn kind(response: &Value) -> &str {
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
async fn polling_an_unknown_task_says_tasks_do_not_outlive_the_instance() {
    let response = Harness::new()
        .call("task.get", json!({ "taskId": "mcp-nope" }))
        .await;

    assert_eq!(kind(&response), "noSuchTask");
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("do not survive"));
}

#[tokio::test]
async fn a_settled_task_carries_its_reply_and_usage() {
    let mut harness = Harness::new();
    let dispatched = harness
        .call("task.dispatch", json!({ "instruction": "x" }))
        .await;
    let task_id = dispatched["result"]["taskId"].as_str().unwrap().to_string();

    let response = harness
        .call("task.get", json!({ "taskId": task_id, "waitSeconds": 5 }))
        .await;

    let result = &response["result"];
    assert_eq!(result["status"], json!("done"));
    assert_eq!(result["reply"], json!("done"));
    assert_eq!(result["usage"]["inputTokens"], json!(10));
}

#[tokio::test]
async fn a_shed_task_is_retryable_and_a_failed_one_is_not() {
    // The distinction a model has to act on: `busy` means nothing was attempted
    // and the same dispatch may well succeed, where a worker error is the task's
    // own failure and retrying just burns another session.
    for (outcome, status, retryable) in [
        (
            FakeOutcome::Fail(crate::hub::RunError::Busy("at capacity".into())),
            "busy",
            true,
        ),
        (
            FakeOutcome::Fail(crate::hub::RunError::Held("operator here".into())),
            "held",
            true,
        ),
        (
            FakeOutcome::Fail(crate::hub::RunError::Worker("it broke".into())),
            "failed",
            false,
        ),
        (
            FakeOutcome::Fail(crate::hub::RunError::Aborted),
            "aborted",
            false,
        ),
    ] {
        let mut harness = Harness::with(
            FakeFleet::new().with_outcome(outcome),
            Grant::new("s", 0, 2),
        );
        let dispatched = harness
            .call("task.dispatch", json!({ "instruction": "x" }))
            .await;
        let task_id = dispatched["result"]["taskId"].as_str().unwrap().to_string();

        let response = harness
            .call("task.get", json!({ "taskId": task_id, "waitSeconds": 5 }))
            .await;

        let result = &response["result"];
        assert_eq!(result["status"], json!(status));
        assert_eq!(result["retryable"], json!(retryable), "for {status}");
    }
}

#[tokio::test]
async fn a_zero_wait_poll_answers_still_running() {
    let mut harness = Harness::with(
        FakeFleet::new().with_outcome(FakeOutcome::Hang),
        Grant::new("s", 0, 2),
    );
    let dispatched = harness
        .call("task.dispatch", json!({ "instruction": "x" }))
        .await;
    let task_id = dispatched["result"]["taskId"].as_str().unwrap().to_string();

    let response = harness
        .call("task.get", json!({ "taskId": task_id, "waitSeconds": 0 }))
        .await;

    assert_eq!(response["result"]["status"], json!("running"));
}

#[tokio::test]
async fn listing_shows_this_sessions_tasks_newest_first() {
    let mut harness = Harness::new();
    for instruction in ["first", "second"] {
        harness
            .call("task.dispatch", json!({ "instruction": instruction }))
            .await;
    }

    let response = harness.call("task.list", json!({})).await;

    let tasks = response["result"]["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 2);
}

#[tokio::test]
async fn aborting_settles_a_hanging_task() {
    let mut harness = Harness::with(
        FakeFleet::new().with_outcome(FakeOutcome::Hang),
        Grant::new("s", 0, 2),
    );
    let dispatched = harness
        .call("task.dispatch", json!({ "instruction": "x" }))
        .await;
    let task_id = dispatched["result"]["taskId"].as_str().unwrap().to_string();

    let aborted = harness
        .call("task.abort", json!({ "taskId": task_id.clone() }))
        .await;
    assert_eq!(aborted["result"]["aborted"], json!(true));

    let response = harness
        .call("task.get", json!({ "taskId": task_id, "waitSeconds": 5 }))
        .await;
    assert_eq!(response["result"]["status"], json!("aborted"));
}

#[tokio::test]
async fn aborting_an_unknown_task_is_refused_rather_than_guessed_at() {
    let response = Harness::new()
        .call("task.abort", json!({ "taskId": "mcp-someone-elses" }))
        .await;

    assert_eq!(kind(&response), "noSuchTask");
}

#[tokio::test]
async fn a_grant_without_the_fleet_family_cannot_dispatch() {
    let mut harness = Harness::with(
        FakeFleet::new(),
        Grant::new("s", 0, 2).with_families(ToolFamilies::workflows_only()),
    );

    let response = harness
        .call("task.dispatch", json!({ "instruction": "x" }))
        .await;

    // Reported apart from the depth ceiling: this one is an operator decision
    // about the whole session, not about where this harness sits in a tree.
    assert_eq!(kind(&response), "unauthenticated");
}

#[tokio::test]
async fn a_grant_at_the_depth_ceiling_is_told_to_do_the_work_itself() {
    let mut harness = Harness::with(FakeFleet::new(), Grant::new("s", 2, 2));

    let response = harness
        .call("task.dispatch", json!({ "instruction": "x" }))
        .await;

    assert_eq!(kind(&response), "depthExceeded");
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("do the work"));
}
