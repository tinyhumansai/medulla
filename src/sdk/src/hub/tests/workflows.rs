//! Tests for the hub's cloud workflow plane: the raw `medulla:workflow_request`
//! frame in, the one `medulla:workflow_result` frame out.
//!
//! The bridge under test is the real [`StoreWorkflowBridge`] over a temporary
//! [`FileWorkflowStore`], because the property that matters is that a socket
//! read answers exactly what the store holds. What is *not* covered here is the
//! Socket.IO transport itself — `rust_socketio` has no in-process server to
//! connect to, so the emit and the `.on(...)` registration in
//! [`crate::hub::socket`] are exercised only by the wiring that hands this
//! module a frame.

use std::sync::Arc;

use serde_json::json;

use crate::hub::workflows::{advert_batch, answer, WorkflowPlane};
use crate::hub::{RunError, TaskOutcome, TaskRequest};
use crate::workflows::{ops, FileWorkflowStore, StoreWorkflowBridge, WorkflowStore};

/// A workflow document with one trigger and one agent step.
fn document(id: &str) -> String {
    json!({
        "id": id,
        "name": "Nightly sweep",
        "description": "sweeps the estate",
        "nodes": [
            { "id": "start", "kind": "trigger", "name": "Start",
              "config": { "trigger_kind": "manual" } },
            { "id": "work", "kind": "agent", "name": "Work",
              "config": { "prompt": "do it", "agent_ref": "builder" } }
        ],
        "edges": [{ "from_node": "start", "to_node": "work" }]
    })
    .to_string()
}

/// A real store in a temporary directory, holding `sweep`.
fn store_with_sweep() -> (tempfile::TempDir, Arc<dyn WorkflowStore>) {
    let root = tempfile::tempdir().expect("tempdir");
    let store: Arc<dyn WorkflowStore> = Arc::new(FileWorkflowStore::new(
        vec![root.path().join("workflows")],
        root.path().join("runs"),
    ));
    ops::create(&store, &document("sweep"), "sweep").expect("creates");
    (root, store)
}

/// The bridge a host installs: the real store adapter over that store.
fn installed() -> (tempfile::TempDir, WorkflowPlane) {
    let (root, store) = store_with_sweep();
    let bridge: WorkflowPlane = Arc::new(StoreWorkflowBridge::new(store).with_agent_id("laptop"));
    (root, bridge)
}

/// A harness stand-in that replies without touching the store, so a copilot
/// turn's plumbing can be exercised without starting a coding agent.
struct StubHarness {
    reply: String,
}

#[async_trait::async_trait]
impl crate::flow_engine::caps::dispatch::HarnessDispatch for StubHarness {
    async fn dispatch(&self, _request: TaskRequest) -> Result<TaskOutcome, RunError> {
        Ok(TaskOutcome {
            reply: self.reply.clone(),
            usage: crate::protocol::TokenUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
            harness: None,
            session_id: None,
            transcript: Vec::new(),
        })
    }
}

/// A bridge whose every method panics.
///
/// Not a stand-in for the store — the store is real everywhere else in this
/// file — but the only way to prove the transport survives a bridge that
/// unwinds, which no well-behaved store can be asked to do on demand.
struct ExplodingBridge;

#[async_trait::async_trait]
impl openhuman_core::openhuman::platform::socket::medulla::workflows::WorkflowBridge
    for ExplodingBridge
{
    fn list(
        &self,
    ) -> Vec<openhuman_core::openhuman::platform::socket::medulla::payloads::WorkflowDescriptor>
    {
        panic!("store exploded");
    }

    fn get(&self, _id: &str) -> Result<serde_json::Value, String> {
        panic!("store exploded");
    }

    fn node_kinds(&self, _kind: Option<&str>) -> Result<serde_json::Value, String> {
        panic!("store exploded");
    }

    fn runs(&self, _id: &str) -> Result<serde_json::Value, String> {
        panic!("store exploded");
    }

    async fn copilot(
        &self,
        _instruction: &str,
        _workflow_id: Option<&str>,
    ) -> Result<
        openhuman_core::openhuman::platform::socket::medulla::payloads::CopilotOutcome,
        String,
    > {
        panic!("copilot exploded");
    }
}

#[tokio::test]
async fn a_get_frame_is_answered_from_the_installed_store() {
    let (_root, bridge) = installed();

    let reply = answer(
        json!({ "requestId": "r1", "op": "get", "workflowId": "sweep" }),
        Some(bridge),
    )
    .await
    .expect("a frame carrying a requestId is always answerable");

    assert_eq!(reply.request_id, "r1");
    assert!(reply.ok, "{reply:?}");
    let data = reply.data.expect("the graph");
    assert_eq!(data["id"], "sweep");
    assert_eq!(data["name"], "Nightly sweep");
    assert!(reply.error.is_none());
}

#[tokio::test]
async fn every_read_op_reaches_the_store_it_names() {
    let (_root, bridge) = installed();

    let kinds = answer(
        json!({ "requestId": "r2", "op": "node_kinds", "kind": "agent" }),
        Some(bridge.clone()),
    )
    .await
    .expect("answerable");
    assert!(kinds.ok, "{kinds:?}");
    assert_eq!(
        kinds.data.expect("the catalog"),
        ops::catalog(Some("agent")).expect("catalog")
    );

    // No `kind` is the whole catalog, not a missing argument.
    let all = answer(
        json!({ "requestId": "r3", "op": "node_kinds" }),
        Some(bridge.clone()),
    )
    .await
    .expect("answerable");
    assert_eq!(all.data.expect("the catalog"), ops::catalog(None).unwrap());

    let runs = answer(
        json!({ "requestId": "r4", "op": "runs", "workflowId": "sweep" }),
        Some(bridge),
    )
    .await
    .expect("answerable");
    assert!(runs.ok, "{runs:?}");
    assert!(runs.data.expect("the run list")["runs"].is_array());
}

#[tokio::test]
async fn a_read_missing_its_workflow_id_is_refused_rather_than_guessed() {
    let (_root, bridge) = installed();

    for op in ["get", "runs"] {
        let reply = answer(json!({ "requestId": "r5", "op": op }), Some(bridge.clone()))
            .await
            .expect("answerable");
        assert!(!reply.ok);
        assert_eq!(
            reply.error.as_deref(),
            Some(format!("workflow {op} requires a workflowId").as_str())
        );
        assert!(reply.data.is_none());
    }

    // Blank is absent, not an id that will merely miss in the store.
    let blank = answer(
        json!({ "requestId": "r6", "op": "get", "workflowId": "  " }),
        Some(bridge),
    )
    .await
    .expect("answerable");
    assert_eq!(
        blank.error.as_deref(),
        Some("workflow get requires a workflowId")
    );
}

#[tokio::test]
async fn an_unknown_workflow_id_is_reported_not_dropped() {
    // Dropping it would cost the backend its whole ten-second read deadline.
    let (_root, bridge) = installed();

    let reply = answer(
        json!({ "requestId": "r7", "op": "get", "workflowId": "missing" }),
        Some(bridge),
    )
    .await
    .expect("answerable");

    assert_eq!(reply.request_id, "r7");
    assert!(!reply.ok);
    assert_eq!(
        reply.error.as_deref(),
        Some("no workflow with id 'missing'")
    );
}

#[tokio::test]
async fn an_op_this_build_cannot_read_is_still_answered() {
    // A newer backend asking for an op that does not exist here, or a frame
    // whose fields are the wrong shape: the `requestId` is recovered from the
    // raw JSON so the waiter settles now instead of on its deadline.
    let (_root, bridge) = installed();

    for raw in [
        json!({ "requestId": "r8", "op": "apply_ops" }),
        json!({ "requestId": "r8", "op": 7 }),
        json!({ "requestId": "r8" }),
    ] {
        let reply = answer(raw.clone(), Some(bridge.clone()))
            .await
            .unwrap_or_else(|| panic!("{raw} carries a requestId and must be answered"));
        assert_eq!(reply.request_id, "r8");
        assert!(!reply.ok);
        let error = reply.error.expect("a reason");
        assert!(
            error.starts_with("this host could not read the request:"),
            "{error}"
        );
    }
}

#[tokio::test]
async fn a_frame_without_a_request_id_is_only_logged() {
    // Nothing to correlate, so nothing can be answered — but it must not panic
    // on the socket path either.
    let (_root, bridge) = installed();

    assert!(answer(json!({ "op": "get" }), Some(bridge.clone()))
        .await
        .is_none());
    assert!(answer(json!("not even an object"), Some(bridge))
        .await
        .is_none());
}

#[tokio::test]
async fn a_host_with_no_workflow_store_refuses_instead_of_going_quiet() {
    let reply = answer(
        json!({ "requestId": "r9", "op": "get", "workflowId": "x" }),
        None,
    )
    .await
    .expect("answerable");

    assert_eq!(reply.request_id, "r9");
    assert!(!reply.ok);
    assert_eq!(
        reply.error.as_deref(),
        Some("this host has no workflow store installed")
    );
}

#[tokio::test]
async fn a_panicking_read_still_answers() {
    // A bridge that unwinds must not take the reply with it: the backend would
    // wait out the whole deadline for an answer that is never coming.
    let bridge: WorkflowPlane = Arc::new(ExplodingBridge);

    let reply = answer(
        json!({ "requestId": "r10", "op": "get", "workflowId": "sweep" }),
        Some(bridge),
    )
    .await
    .expect("answerable");

    assert_eq!(reply.request_id, "r10");
    assert!(!reply.ok);
    assert!(
        reply
            .error
            .as_deref()
            .is_some_and(|e| e.starts_with("the workflow store failed to answer")),
        "{reply:?}"
    );
}

#[tokio::test]
async fn a_copilot_turn_answers_with_the_hosts_own_outcome() {
    let (_root, store) = store_with_sweep();
    let harness = Arc::new(StubHarness {
        reply: "  looked at it  ".into(),
    });
    let bridge: WorkflowPlane =
        Arc::new(StoreWorkflowBridge::new(store).with_copilot(harness, "worker", None, None));

    let reply = answer(
        json!({ "requestId": "r11", "op": "copilot", "workflowId": "sweep",
                "instruction": "what does it do?" }),
        Some(bridge),
    )
    .await
    .expect("answerable");

    assert!(reply.ok, "{reply:?}");
    let data = reply.data.expect("the outcome");
    assert_eq!(data["reply"], "looked at it");
    // Derived from a re-read of the store: the stub edited nothing, so the
    // outcome must claim nothing.
    assert_eq!(data["changes"], json!([]));
    assert!(data.get("created").is_none());
}

#[tokio::test]
async fn a_copilot_request_without_an_instruction_never_reaches_the_agent() {
    let (_root, bridge) = installed();

    for raw in [
        json!({ "requestId": "r12", "op": "copilot" }),
        json!({ "requestId": "r12", "op": "copilot", "instruction": "   " }),
    ] {
        let reply = answer(raw, Some(bridge.clone())).await.expect("answerable");
        assert!(!reply.ok);
        assert_eq!(
            reply.error.as_deref(),
            Some("workflow copilot requires a non-empty instruction")
        );
    }
}

#[tokio::test]
async fn a_panicking_copilot_still_answers() {
    // The expensive one to drop: a copilot request the backend never hears back
    // about holds a promise open for ten minutes.
    let bridge: WorkflowPlane = Arc::new(ExplodingBridge);

    let reply = answer(
        json!({ "requestId": "r13", "op": "copilot", "instruction": "add a step" }),
        Some(bridge),
    )
    .await
    .expect("answerable");

    assert!(!reply.ok);
    assert!(
        reply
            .error
            .as_deref()
            .is_some_and(|e| e.starts_with("the workflow copilot failed to answer")),
        "{reply:?}"
    );
}

#[tokio::test]
async fn the_advert_batch_carries_the_store_and_the_batch_provenance() {
    // What the hub emits as `medulla:register_workflows` on every (re)connect.
    let (_root, bridge) = installed();

    let batch = advert_batch(&bridge).await.expect("the store is readable");

    assert_eq!(batch.workflows.len(), 1);
    assert_eq!(batch.workflows[0].id, "sweep");
    assert_eq!(batch.workflows[0].name, "Nightly sweep");
    assert_eq!(batch.workflows[0].node_count, 2);
    assert_eq!(batch.agent_id.as_deref(), Some("laptop"));
    // Serialized, it is the frame the backend's `HarnessRegisterWorkflowsPayload`
    // reads: a `workflows` array and an optional batch-level `agentId`.
    let wire = serde_json::to_value(&batch).expect("serializes");
    assert_eq!(wire["workflows"][0]["nodeCount"], 2);
    assert_eq!(wire["agentId"], "laptop");
}

#[tokio::test]
async fn a_panicking_list_advertises_nothing_rather_than_taking_the_connect_down() {
    // The advert is read on connect, inside the socket's own callback; an
    // unwinding store there would poison the connection the hub needs for tasks.
    let bridge: WorkflowPlane = Arc::new(ExplodingBridge);

    assert!(advert_batch(&bridge).await.is_none());
}
