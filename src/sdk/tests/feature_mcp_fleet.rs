#![cfg(unix)]

//! The fleet plane wired as it ships: a real control server, a real proxy
//! backend, and the real MCP request handler, with only the fleet itself faked.
//!
//! The unit tests either side of the socket call their halves directly, which
//! cannot catch a framing or correlation mismatch *between* them — both sides
//! can be individually correct and still fail to talk. This is the test that
//! would notice.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::sync::mpsc;

use medulla::control_socket::{
    ControlServer, FleetOps, FleetWorker, Grant, GrantRegistry, ToolFamilies,
};
use medulla::hub::{RunError, TaskOutcome, TaskRequest};
use medulla::mcp::{backend::ProxyFleet, handle_request, McpSession, ToolMode};
use medulla::tinyplace::TokenUsage;
use medulla::workflows::{FileWorkflowStore, WorkflowStore};

/// A fleet that settles every dispatch with a reply, recording what it got.
struct RecordingFleet {
    dispatched: Mutex<Vec<TaskRequest>>,
}

#[async_trait::async_trait]
impl FleetOps for RecordingFleet {
    fn workers(&self) -> Option<Vec<FleetWorker>> {
        Some(vec![FleetWorker {
            id: "alpha".into(),
            address: "alpha-address".into(),
            harness: "claude".into(),
            selected: true,
            ..FleetWorker::default()
        }])
    }

    fn default_worker(&self) -> Option<String> {
        Some("alpha-address".into())
    }

    async fn dispatch(
        &self,
        request: TaskRequest,
        _status: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<TaskOutcome, RunError> {
        self.dispatched.lock().unwrap().push(request);
        Ok(TaskOutcome {
            reply: "the work is done".into(),
            usage: TokenUsage {
                input_tokens: 11,
                output_tokens: 22,
            },
            harness: None,
        })
    }

    fn abort(&self, _abort_id: &str) {}
}

/// Bring up a server and an MCP session talking to it over a real socket.
async fn wired(
    grant: Grant,
) -> (
    tempfile::TempDir,
    ControlServer,
    Arc<RecordingFleet>,
    McpSession,
) {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("control.sock");
    let grants = GrantRegistry::new();
    let token = grants.mint(grant);
    let fleet = Arc::new(RecordingFleet {
        dispatched: Mutex::new(Vec::new()),
    });
    let ops: Arc<dyn FleetOps> = fleet.clone();
    let server = ControlServer::bind(&socket, ops, grants, false)
        .await
        .expect("the control socket binds");

    let store: Arc<dyn WorkflowStore> = Arc::new(FileWorkflowStore::new(
        vec![dir.path().join("workflows")],
        dir.path().join("runs"),
    ));
    let proxy = ProxyFleet::connect(&socket, &token)
        .await
        .expect("the shim connects");
    let session =
        McpSession::local(store, Default::default(), ToolMode::Full).with_fleet(Arc::new(proxy));

    (dir, server, fleet, session)
}

/// The tool names this session advertises.
async fn advertised(session: &McpSession) -> Vec<String> {
    let request = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
    let response = handle_request(session, &request).await.expect("a response");
    response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_string())
        .collect()
}

/// Call a tool and return its payload plus the error flag.
async fn call(session: &McpSession, name: &str, arguments: Value) -> (Value, bool) {
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": name, "arguments": arguments },
    });
    let response = handle_request(session, &request).await.expect("a response");
    let result = &response["result"];
    let text = result["content"][0]["text"].as_str().expect("text content");
    (
        serde_json::from_str(text).expect("tool results are JSON"),
        result["isError"].as_bool().unwrap_or(false),
    )
}

#[tokio::test]
async fn a_dispatch_crosses_the_socket_and_comes_back_as_a_reply() {
    let (_dir, _server, fleet, session) = wired(Grant::new("session", 0, 2)).await;

    let (dispatched, is_error) = call(
        &session,
        "fleet_dispatch",
        json!({ "instruction": "fix the flaky test" }),
    )
    .await;
    assert!(!is_error, "{dispatched}");
    let task_id = dispatched["taskId"].as_str().unwrap().to_string();

    let (settled, is_error) = call(
        &session,
        "fleet_result",
        json!({ "taskId": task_id, "waitSeconds": 5 }),
    )
    .await;

    assert!(!is_error, "{settled}");
    assert_eq!(settled["status"], json!("done"));
    assert_eq!(settled["reply"], json!("the work is done"));
    // Usage reaches the model, so it can see what a dispatch cost.
    assert_eq!(settled["usage"]["inputTokens"], json!(11));

    let requests = fleet.dispatched.lock().unwrap();
    let request = requests.first().expect("the fleet ran it");
    assert_eq!(request.instruction, "fix the flaky test");
    assert_eq!(request.worker_address, "alpha-address");
    // Context-free, and one level below the dispatcher.
    assert_eq!(request.conversation, None);
    assert_eq!(request.fleet_depth, 1);
}

#[tokio::test]
async fn an_out_of_range_result_wait_cannot_overflow_the_proxy_deadline() {
    let (_dir, _server, _fleet, session) = wired(Grant::new("session", 0, 2)).await;
    let (dispatched, is_error) = call(
        &session,
        "fleet_dispatch",
        json!({ "instruction": "finish immediately" }),
    )
    .await;
    assert!(!is_error, "{dispatched}");

    let (settled, is_error) = call(
        &session,
        "fleet_result",
        json!({
            "taskId": dispatched["taskId"],
            "waitSeconds": u64::MAX,
        }),
    )
    .await;

    assert!(!is_error, "{settled}");
    assert_eq!(settled["status"], json!("done"));
}

#[tokio::test]
async fn the_roster_survives_the_round_trip_intact() {
    let (_dir, _server, _fleet, session) = wired(Grant::new("session", 0, 2)).await;

    let (workers, is_error) = call(&session, "fleet_workers", json!({})).await;

    assert!(!is_error, "{workers}");
    assert_eq!(workers["workers"][0]["id"], json!("alpha"));
    assert_eq!(workers["workers"][0]["harness"], json!("claude"));
    assert_eq!(workers["defaultWorker"], json!("alpha-address"));
}

#[tokio::test]
async fn a_grant_at_the_ceiling_dispatches_nothing_over_the_wire() {
    // The end-to-end version of the depth guard: the verb is not advertised,
    // and the fleet never sees a request even if one is forced through.
    let (_dir, _server, fleet, session) = wired(Grant::new("session", 2, 2)).await;

    let names = advertised(&session).await;
    assert!(!names.contains(&"fleet_dispatch".to_string()));
    assert!(names.contains(&"fleet_status".to_string()));

    let (refusal, is_error) = call(
        &session,
        "fleet_dispatch",
        json!({ "instruction": "fan out anyway" }),
    )
    .await;

    assert!(is_error);
    assert!(refusal["error"].as_str().unwrap().contains("do the work"));
    assert!(fleet.dispatched.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_workflows_only_grant_gets_no_fleet_surface_at_all() {
    let (_dir, _server, _fleet, session) =
        wired(Grant::new("session", 0, 2).with_families(ToolFamilies::workflows_only())).await;

    let names = advertised(&session).await;

    assert!(!names.iter().any(|name| name.starts_with("fleet_")));
    assert!(names.iter().any(|name| name.starts_with("workflow_")));
}

#[tokio::test]
async fn a_settled_dispatch_frees_its_in_flight_slot_across_the_socket() {
    let (_dir, _server, _fleet, session) =
        wired(Grant::new("session", 0, 2).with_max_in_flight(1)).await;

    // The recording fleet settles immediately, so the first dispatch is done
    // before the second is asked for — the ceiling is on *in-flight* work, and
    // a settled task must not count against it.
    for instruction in ["first", "second", "third"] {
        let (result, is_error) = call(
            &session,
            "fleet_dispatch",
            json!({ "instruction": instruction }),
        )
        .await;
        assert!(!is_error, "a settled task should free its slot: {result}");
    }
}

#[tokio::test]
async fn workflow_tools_keep_working_alongside_the_fleet_ones() {
    // The two families share one server; adding the fleet must not disturb the
    // surface that was already there.
    let (_dir, _server, _fleet, session) = wired(Grant::new("session", 0, 2)).await;

    let (listed, is_error) = call(&session, "workflow_list", json!({})).await;

    assert!(!is_error, "{listed}");
    assert!(listed.get("workflows").is_some(), "{listed}");
}
