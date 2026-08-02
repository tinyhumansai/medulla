//! Fleet-tool behaviour: what each session is advertised, what reaches the
//! control plane, and how a refusal reads by the time a model sees it.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::control_socket::{ControlError, ControlFailure, ErrorKind, Hello, ToolFamilies};
use crate::mcp::backend::FleetBackend;
use crate::mcp::{handle_request, McpSession, OfflineFleet, ToolMode};
use crate::workflows::{FileWorkflowStore, WorkflowStore};

use super::FLEET_TOOL_NAMES;

/// A fleet backend that records what it was asked and answers as told.
struct FakeBackend {
    hello: Option<Hello>,
    answer: Result<Value, ControlError>,
    /// Every `(op, params)` that reached the control plane.
    calls: Mutex<Vec<(String, Value)>>,
}

impl FakeBackend {
    /// A connected fleet whose grant may dispatch.
    fn connected() -> Self {
        FakeBackend {
            hello: Some(Hello {
                protocol: 1,
                version: "0.5.6".into(),
                hub_ready: true,
                depth: 0,
                max_depth: 2,
                max_in_flight: 4,
                families: ToolFamilies::default(),
            }),
            answer: Ok(json!({ "ok": true })),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// A connected fleet at its depth ceiling.
    fn at_depth_ceiling() -> Self {
        let mut backend = Self::connected();
        if let Some(hello) = backend.hello.as_mut() {
            hello.depth = 2;
        }
        backend
    }

    /// Answer every call with `error` instead.
    fn refusing(error: ControlError) -> Self {
        let mut backend = Self::connected();
        backend.answer = Err(error);
        backend
    }
}

#[async_trait::async_trait]
impl FleetBackend for FakeBackend {
    fn hello(&self) -> Option<&Hello> {
        self.hello.as_ref()
    }

    async fn call(&self, op: &str, params: Value) -> Result<Value, ControlError> {
        self.calls.lock().unwrap().push((op.to_string(), params));
        self.answer.clone()
    }
}

/// A session over a scratch store, backed by `fleet`.
fn session(fleet: Arc<dyn FleetBackend>) -> (tempfile::TempDir, McpSession) {
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn WorkflowStore> = Arc::new(FileWorkflowStore::new(
        vec![root.path().join("workflows")],
        root.path().join("runs"),
    ));
    let session = McpSession::local(store, Default::default(), ToolMode::Full).with_fleet(fleet);
    (root, session)
}

/// The tool names this session advertises.
async fn advertised(session: &McpSession) -> Vec<String> {
    let request = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });
    let response = handle_request(session, &request).await.expect("a response");
    response["result"]["tools"]
        .as_array()
        .expect("a tool list")
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_string())
        .collect()
}

/// Call one tool and return its payload plus the error flag.
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
async fn a_session_with_no_fleet_is_shown_no_fleet_tools() {
    // Not advertised-and-always-failing: a tool that is absent is a fact a model
    // cannot argue with, where one that is present and refuses every call reads
    // as a broken server and invites retries.
    let (_root, session) = session(Arc::new(OfflineFleet));

    let names = advertised(&session).await;

    for tool in FLEET_TOOL_NAMES {
        assert!(!names.contains(&tool.to_string()), "{tool} was advertised");
    }
    assert!(names.iter().any(|name| name.starts_with("workflow_")));
}

#[tokio::test]
async fn a_fleet_only_host_stays_closed_when_its_grant_cannot_connect() {
    let root = tempfile::tempdir().unwrap();
    let store: Arc<dyn WorkflowStore> = Arc::new(FileWorkflowStore::new(
        vec![root.path().join("workflows")],
        root.path().join("runs"),
    ));
    let session = McpSession::local(store, Default::default(), ToolMode::Full)
        .with_workflows_enabled(false)
        .with_fleet(Arc::new(OfflineFleet));

    let names = advertised(&session).await;

    assert!(
        names.is_empty(),
        "an offline fleet-only grant must fail closed"
    );
}

#[tokio::test]
async fn a_connected_session_is_shown_every_fleet_tool() {
    let (_root, session) = session(Arc::new(FakeBackend::connected()));

    let names = advertised(&session).await;

    for tool in FLEET_TOOL_NAMES {
        assert!(names.contains(&tool.to_string()), "{tool} was missing");
    }
}

#[tokio::test]
async fn at_the_depth_ceiling_the_verbs_that_start_work_are_withheld() {
    // The guard is the withheld verb, not a refusal: a standing instruction is
    // something one confused turn talks itself past, an absent tool is not.
    let (_root, session) = session(Arc::new(FakeBackend::at_depth_ceiling()));

    let names = advertised(&session).await;

    assert!(!names.contains(&"fleet_dispatch".to_string()));
    assert!(!names.contains(&"fleet_abort".to_string()));
    assert!(!names.contains(&"fleet_result".to_string()));
    // Reading is still allowed: knowing what the fleet is doing never fans out.
    assert!(names.contains(&"fleet_status".to_string()));
    assert!(names.contains(&"fleet_workers".to_string()));
    assert!(names.contains(&"fleet_tasks".to_string()));
}

#[tokio::test]
async fn status_refreshes_hub_readiness_after_the_handshake() {
    let mut backend = FakeBackend::connected();
    backend.hello.as_mut().unwrap().hub_ready = false;
    let backend = Arc::new(backend);
    let (_root, session) = session(backend.clone());

    let (payload, is_error) = call(&session, "fleet_status", json!({})).await;

    assert!(!is_error);
    assert_eq!(payload["connected"], json!(true));
    assert_eq!(payload["hubReady"], json!(true));
    assert_eq!(payload["mayDispatch"], json!(true));
    assert_eq!(payload["maxDepth"], json!(2));
    assert_eq!(
        backend.calls.lock().unwrap().as_slice(),
        [("worker.list".to_string(), json!({}))]
    );
}

#[tokio::test]
async fn status_reports_a_still_connecting_hub_without_failing() {
    let backend = Arc::new(FakeBackend::refusing(ControlError::Refused(
        ControlFailure::new(ErrorKind::HubNotReady, "still connecting"),
    )));
    let (_root, session) = session(backend);

    let (payload, is_error) = call(&session, "fleet_status", json!({})).await;

    assert!(!is_error);
    assert_eq!(payload["connected"], json!(true));
    assert_eq!(payload["hubReady"], json!(false));
}

#[tokio::test]
async fn calling_a_fleet_tool_without_a_grant_is_refused_readably() {
    // A session with no grant never sees these verbs (see the advertising test
    // above), so reaching here means the model called one it was not offered.
    // It still gets prose rather than a protocol error the client would swallow.
    let (_root, session) = session(Arc::new(OfflineFleet));

    let (payload, is_error) = call(&session, "fleet_status", json!({})).await;

    assert!(is_error);
    assert!(payload["error"]
        .as_str()
        .unwrap()
        .contains("not available to this session"));
}

#[tokio::test]
async fn the_offline_status_payload_tells_a_model_to_do_the_work_itself() {
    // The rendering used when a session *does* hold the family but its backend
    // has no handshake — reachable if a fleet is granted and the socket dies.
    let payload = super::status(&McpSession::local(
        Arc::new(FileWorkflowStore::new(vec![], std::path::PathBuf::new())),
        Default::default(),
        ToolMode::Full,
    ))
    .await
    .unwrap();

    assert_eq!(payload["connected"], json!(false));
    assert_eq!(payload["mayDispatch"], json!(false));
    assert!(payload["detail"].as_str().unwrap().contains("do the work"));
}

#[tokio::test]
async fn a_dispatch_forwards_only_the_hints_a_model_may_choose() {
    let backend = Arc::new(FakeBackend::connected());
    let (_root, session) = session(backend.clone());

    call(
        &session,
        "fleet_dispatch",
        json!({
            "instruction": "fix the flaky test",
            "worker": "alpha",
            "harness": "claude",
            "workflow": "release",
            "workflowFingerprint": "release-fingerprint",
            "inputs": { "environment": "staging", "retries": 2 },
            // Capability handles keyed into registries shared by every dispatch.
            // A model that picks these could dedupe or cancel somebody else's
            // work, so they must not reach the wire.
            "taskId": "chosen",
            "abortId": "chosen",
            "conversation": "shared",
            "toolMode": "full",
        }),
    )
    .await;

    let calls = backend.calls.lock().unwrap();
    let (op, params) = calls.first().expect("a dispatch reached the control plane");
    assert_eq!(op, "task.dispatch");
    assert_eq!(params["instruction"], json!("fix the flaky test"));
    assert_eq!(params["worker"], json!("alpha"));
    assert_eq!(params["harness"], json!("claude"));
    assert_eq!(params["workflow"], json!("release"));
    assert_eq!(params["workflowFingerprint"], json!("release-fingerprint"));
    assert_eq!(
        params["inputs"],
        json!({ "environment": "staging", "retries": 2 })
    );
    for forged in ["taskId", "abortId", "conversation", "toolMode"] {
        assert!(params.get(forged).is_none(), "{forged} reached the wire");
    }
}

#[tokio::test]
async fn a_dispatch_without_an_instruction_is_a_protocol_error() {
    // Matching the workflow family: a missing required argument is a real
    // JSON-RPC error, not readable content, because there is no partial request
    // to honour.
    let (_root, session) = session(Arc::new(FakeBackend::connected()));
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "fleet_dispatch", "arguments": {} },
    });

    let response = handle_request(&session, &request)
        .await
        .expect("a response");

    assert_eq!(response["error"]["code"], json!(-32602));
}

#[tokio::test]
async fn a_result_poll_defaults_to_a_wait_inside_a_client_timeout() {
    let backend = Arc::new(FakeBackend::connected());
    let (_root, session) = session(backend.clone());

    call(&session, "fleet_result", json!({ "taskId": "mcp-1" })).await;

    let calls = backend.calls.lock().unwrap();
    let (op, params) = calls.first().expect("a poll");
    assert_eq!(op, "task.get");
    assert_eq!(params["taskId"], json!("mcp-1"));
    assert_eq!(params["waitSeconds"], json!(25));
}

#[tokio::test]
async fn a_caller_may_shorten_the_wait_to_a_bare_poll() {
    let backend = Arc::new(FakeBackend::connected());
    let (_root, session) = session(backend.clone());

    call(
        &session,
        "fleet_result",
        json!({ "taskId": "mcp-1", "waitSeconds": 0 }),
    )
    .await;

    assert_eq!(backend.calls.lock().unwrap()[0].1["waitSeconds"], json!(0));
}

#[tokio::test]
async fn a_retryable_refusal_says_so_and_a_permanent_one_does_not() {
    // The distinction a model has to act on. Told only "it failed", it either
    // abandons work that never started or hammers something that never will.
    let retryable = Arc::new(FakeBackend::refusing(ControlError::Refused(
        ControlFailure::new(ErrorKind::HubNotReady, "the fleet is still connecting"),
    )));
    let (_retryable_root, retryable_session) = session(retryable);
    let (payload, is_error) = call(&retryable_session, "fleet_workers", json!({})).await;
    assert!(is_error);
    assert!(payload["error"].as_str().unwrap().contains("trying again"));

    let permanent = Arc::new(FakeBackend::refusing(ControlError::Refused(
        ControlFailure::new(ErrorKind::DepthExceeded, "you are two levels deep"),
    )));
    let (_permanent_root, permanent_session) = session(permanent);
    let (payload, is_error) = call(&permanent_session, "fleet_workers", json!({})).await;
    assert!(is_error);
    assert!(!payload["error"].as_str().unwrap().contains("trying again"));
}

#[tokio::test]
async fn a_fleet_failure_is_readable_content_not_a_protocol_error() {
    // MCP reports a *tool's* failure as content with `isError`. A protocol error
    // is swallowed by the client, so the model never learns what to do instead.
    let (_root, session) = session(Arc::new(FakeBackend::refusing(ControlError::NoInstance)));

    let (payload, is_error) = call(&session, "fleet_tasks", json!({})).await;

    assert!(is_error);
    assert!(payload["error"].as_str().unwrap().contains("do the work"));
}

#[test]
fn no_fleet_tool_name_collides_with_a_harness_reserved_name() {
    for name in FLEET_TOOL_NAMES {
        assert!(
            !crate::harness_contract::is_reserved_tool_name(name),
            "{name} collides with a harness's own tool"
        );
    }
}

#[tokio::test]
async fn every_advertised_fleet_tool_is_one_the_dispatch_handles() {
    let (_root, session) = session(Arc::new(FakeBackend::connected()));

    for name in advertised(&session).await {
        if !name.starts_with("fleet_") {
            continue;
        }
        assert!(
            FLEET_TOOL_NAMES.contains(&name.as_str()),
            "{name} is advertised but not a known fleet tool"
        );
        let request = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": name, "arguments": {} },
        });
        let response = handle_request(&session, &request)
            .await
            .expect("a response");
        let message = response["error"]["message"].as_str().unwrap_or_default();
        assert!(
            !message.contains("unknown fleet tool"),
            "{name} is advertised but the dispatch does not handle it"
        );
    }
}
