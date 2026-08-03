//! Focused tests for folding ACP stream updates into semantic harness events.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::daemon::providers::{Abort, RunTaskOptions};
use crate::daemon::status_detail;
use crate::protocol::HarnessProvider;
use crate::sessions::WorkspaceContext;

use super::types::FoldState;

#[cfg(all(feature = "workflows", unix))]
struct NoFleet;

#[cfg(all(feature = "workflows", unix))]
#[async_trait::async_trait]
impl crate::control_socket::FleetOps for NoFleet {
    fn workers(&self) -> Option<Vec<crate::control_socket::FleetWorker>> {
        Some(Vec::new())
    }

    fn default_worker(&self) -> Option<String> {
        None
    }

    async fn dispatch(
        &self,
        _request: crate::hub::TaskRequest,
        _status: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<crate::hub::TaskOutcome, crate::hub::RunError> {
        unreachable!("grant exchange never dispatches")
    }

    fn abort(&self, _abort_id: &str) -> bool {
        false
    }
}

#[cfg(all(feature = "workflows", unix))]
#[tokio::test]
async fn a_parent_handoff_is_exchanged_before_the_mcp_server_is_attached() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("control.sock");
    let grants = crate::control_socket::GrantRegistry::new();
    let parent_token = grants.mint(crate::control_socket::Grant::new("parent", 1, 3));
    let ops: Arc<dyn crate::control_socket::FleetOps> = Arc::new(NoFleet);
    let _server = crate::control_socket::ControlServer::bind(&path, ops, grants)
        .await
        .unwrap();
    let env = HashMap::from([
        (
            crate::control_socket::MCP_PARENT_SOCKET_ENV.to_string(),
            path.to_string_lossy().into_owned(),
        ),
        (
            crate::control_socket::MCP_PARENT_GRANT_ENV.to_string(),
            parent_token.clone(),
        ),
    ]);

    let servers = super::execution::medulla_mcp_servers(None, "child", &env).await;
    let agent_client_protocol::schema::v1::McpServer::Stdio(server) = &servers[0] else {
        panic!("Medulla MCP server must use stdio");
    };
    let child_token = server
        .env
        .iter()
        .find(|var| var.name == crate::control_socket::MCP_GRANT_ENV)
        .map(|var| var.value.clone())
        .expect("child grant attached");
    let child = crate::control_socket::ControlClient::connect(&path, &child_token)
        .await
        .unwrap();

    assert_ne!(child_token, parent_token);
    assert_eq!(child.hello().depth, 2);
    assert_eq!(child.hello().max_depth, 3);
}

#[cfg(feature = "workflows")]
#[test]
fn session_grants_read_depth_from_the_task_environment() {
    let env = HashMap::from([(
        crate::control_socket::FLEET_DEPTH_ENV.to_string(),
        "2".to_string(),
    )]);

    let grant = super::execution::session_grant("nested", &env, Some("propose:demo"), true, 3, 5);

    assert_eq!(grant.depth, 2);
    assert_eq!(grant.max_depth, 3);
    assert_eq!(grant.max_in_flight, 5);
    assert_eq!(grant.tool_mode.as_deref(), Some("propose:demo"));
}

#[cfg(feature = "workflows")]
#[test]
fn disabling_workflows_keeps_the_fleet_family_on_the_session_grant() {
    let grant = super::execution::session_grant("fleet-only", &HashMap::new(), None, false, 2, 4);

    assert!(!grant.families.workflows);
    assert!(grant.families.fleet);
}

#[test]
fn agent_message_chunks_form_one_reply() {
    let mut state = FoldState::new(None);
    for text in ["hello ", "world"] {
        let update = serde_json::from_value(serde_json::json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": text}
        }))
        .unwrap();
        state.fold(update);
    }
    assert_eq!(state.reply(), "hello world");
}

#[test]
fn non_text_updates_do_not_pollute_the_reply() {
    let mut state = FoldState::new(None);
    let update = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "call-1",
        "title": "Run tests",
        "kind": "execute",
        "status": "pending"
    }))
    .unwrap();
    state.fold(update);
    assert_eq!(
        state.reply(),
        "ACP agent completed without a text response."
    );
}

#[test]
fn tool_updates_preserve_failure_state_for_the_copilot() {
    let details = Arc::new(Mutex::new(Vec::new()));
    let captured = details.clone();
    let mut state = FoldState::new(Some(Box::new(move |event| {
        if let Some(detail) = status_detail(&event.event) {
            captured.lock().unwrap().push(detail);
        }
    })));
    let call = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "call-1",
        "title": "Terminal",
        "kind": "execute",
        "status": "in_progress",
        "rawInput": { "command": "cargo test --workspace" }
    }))
    .unwrap();
    let failure = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": "call-1",
        "status": "failed",
        "rawOutput": "tests failed"
    }))
    .unwrap();
    let still_running = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": "call-1",
        "status": "in_progress"
    }))
    .unwrap();

    state.fold(call);
    state.fold(still_running);
    state.fold(failure);

    assert_eq!(
        *details.lock().unwrap(),
        [
            "running Terminal · $ cargo test --workspace\u{1f}call-1",
            "tool failed\u{1f}call-1"
        ]
    );
}

#[test]
fn running_tool_patch_surfaces_the_command_when_it_arrives_late() {
    let details = Arc::new(Mutex::new(Vec::new()));
    let captured = details.clone();
    let mut state = FoldState::new(Some(Box::new(move |event| {
        if let Some(detail) = status_detail(&event.event) {
            captured.lock().unwrap().push(detail);
        }
    })));
    let call = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "call-1",
        "title": "Terminal",
        "kind": "execute",
        "status": "in_progress"
    }))
    .unwrap();
    let input = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": "call-1",
        "status": "in_progress",
        "rawInput": { "command": "cargo test --workspace" }
    }))
    .unwrap();
    assert_eq!(
        serde_json::to_value(&input).unwrap()["rawInput"]["command"],
        "cargo test --workspace"
    );

    state.fold(call);
    state.fold(input);

    assert_eq!(
        *details.lock().unwrap(),
        [
            "running Terminal\u{1f}call-1",
            "running Terminal · $ cargo test --workspace\u{1f}call-1"
        ]
    );
}

#[test]
fn running_tool_patch_preserves_initial_metadata() {
    let details = Arc::new(Mutex::new(Vec::new()));
    let captured = details.clone();
    let mut state = FoldState::new(Some(Box::new(move |event| {
        if let Some(detail) = status_detail(&event.event) {
            captured.lock().unwrap().push(detail);
        }
    })));
    let call = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "call-1",
        "title": "Read configuration",
        "kind": "read",
        "status": "in_progress"
    }))
    .unwrap();
    let input = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": "call-1",
        "status": "in_progress",
        "rawInput": { "path": "/tmp/medulla.json" }
    }))
    .unwrap();

    state.fold(call);
    state.fold(input);

    assert_eq!(
        *details.lock().unwrap(),
        [
            "running Read: Read configuration\u{1f}call-1",
            "running Read · /tmp/medulla.json\u{1f}call-1"
        ]
    );
}

#[test]
fn thought_chunks_emit_a_cumulative_bounded_snapshot() {
    let thoughts = Arc::new(Mutex::new(Vec::new()));
    let captured = thoughts.clone();
    let mut state = FoldState::new(Some(Box::new(move |event| {
        if event.event.kind == "agent_thought" {
            captured
                .lock()
                .unwrap()
                .push(event.event.payload["text"].as_str().unwrap().to_string());
        }
    })));
    for text in ["Checking ", "the workflow.", &"x".repeat(1_000)] {
        let update = serde_json::from_value(serde_json::json!({
            "sessionUpdate": "agent_thought_chunk",
            "content": { "type": "text", "text": text }
        }))
        .unwrap();
        state.fold(update);
    }

    let thoughts = thoughts.lock().unwrap();
    assert_eq!(thoughts[0], "Checking ");
    assert_eq!(thoughts[1], "Checking the workflow.");
    assert_eq!(thoughts[2].chars().count(), 780);
    assert!(thoughts[2].starts_with('…'));
}

#[test]
fn usage_updates_do_not_reset_the_cumulative_thought_snapshot() {
    let thoughts = Arc::new(Mutex::new(Vec::new()));
    let captured = thoughts.clone();
    let mut state = FoldState::new(Some(Box::new(move |event| {
        if event.event.kind == "agent_thought" {
            captured
                .lock()
                .unwrap()
                .push(event.event.payload["text"].as_str().unwrap().to_string());
        }
    })));
    for update in [
        serde_json::json!({
            "sessionUpdate": "agent_thought_chunk",
            "content": { "type": "text", "text": "Checking " }
        }),
        serde_json::json!({
            "sessionUpdate": "usage_update",
            "used": 42,
            "size": 100
        }),
        serde_json::json!({
            "sessionUpdate": "agent_thought_chunk",
            "content": { "type": "text", "text": "the workflow." }
        }),
    ] {
        state.fold(serde_json::from_value(update).unwrap());
    }

    assert_eq!(
        *thoughts.lock().unwrap(),
        ["Checking ", "Checking the workflow."]
    );
}

#[test]
fn thought_credentials_are_redacted_before_the_snapshot_is_bounded() {
    let thoughts = Arc::new(Mutex::new(Vec::new()));
    let captured = thoughts.clone();
    let mut state = FoldState::new(Some(Box::new(move |event| {
        if event.event.kind == "agent_thought" {
            captured
                .lock()
                .unwrap()
                .push(event.event.payload["text"].as_str().unwrap().to_string());
        }
    })));
    let prefix = format!("{}sk-", "context ".repeat(110));
    for text in [prefix.as_str(), "abcdefghijklmnop0123456789"] {
        let update = serde_json::from_value(serde_json::json!({
            "sessionUpdate": "agent_thought_chunk",
            "content": { "type": "text", "text": text }
        }))
        .unwrap();
        state.fold(update);
    }

    let final_thought = thoughts.lock().unwrap().last().unwrap().clone();
    assert!(final_thought.contains("[REDACTED]"));
    assert!(!final_thought.contains("0123456789"));
}

// ---------------------------------------------------------------------------
// Attribution reaches the ACP spawn path
// ---------------------------------------------------------------------------

/// A `RunTaskOptions` carrying `attribution`, with everything else inert.
fn attribution_options(attribution: bool) -> RunTaskOptions {
    RunTaskOptions {
        conversation: String::new(),
        session_class: crate::sessions::SessionClass::Bounded,
        resume_session_id: None,
        workspace_context: Default::default(),
        provider: HarnessProvider::Claude,
        prompt: String::new(),
        cwd: ".".to_string(),
        env: HashMap::new(),
        timeout_ms: 1_000,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
        abort: Abort::new(),
        router: None,
        attribution,
        on_event: None,
        on_stdin: None,
        on_session: None,
        on_workspace_context: None,
    }
}

/// `run_provider_task` dispatches to ACP *before* the spawn seam that applies
/// attribution for direct runs, so the ACP agent env must carry it itself —
/// otherwise every ACP-backed commit is unattributed.
#[cfg(unix)]
#[test]
fn agent_env_carries_attribution() {
    let env = super::execution::acp_env(&attribution_options(true));
    assert!(
        env.contains_key("MEDULLA_ATTRIBUTION"),
        "ACP agent env must carry the attribution trailer"
    );
    assert_eq!(
        env.get("GIT_CONFIG_KEY_0").map(String::as_str),
        Some("core.hooksPath"),
        "ACP agent env must activate the hook directory"
    );
}

/// Turning attribution off leaves the ACP env untouched.
#[test]
fn agent_env_omits_attribution_when_off() {
    let env = super::execution::acp_env(&attribution_options(false));
    assert!(!env.contains_key("MEDULLA_ATTRIBUTION"));
    assert!(!env.contains_key("GIT_CONFIG_KEY_0"));
}

#[test]
fn agent_env_strips_inherited_fleet_capabilities() {
    let mut options = attribution_options(false);
    options.env.insert(
        crate::control_socket::MCP_SOCKET_ENV.to_string(),
        "/tmp/another-session.sock".to_string(),
    );
    options.env.insert(
        crate::control_socket::MCP_GRANT_ENV.to_string(),
        "another-session-token".to_string(),
    );

    let env = super::execution::acp_env(&options);

    assert!(!env.contains_key(crate::control_socket::MCP_SOCKET_ENV));
    assert!(!env.contains_key(crate::control_socket::MCP_GRANT_ENV));
}

#[test]
fn acp_pr_correlation_requires_success_in_the_dispatch_workspace() {
    fn update(
        kind: &str,
        status: &str,
        value: serde_json::Value,
    ) -> agent_client_protocol::schema::v1::SessionUpdate {
        let mut update = serde_json::json!({
            "sessionUpdate": kind,
            "toolCallId": "call-pr",
            "title": "Terminal",
            "kind": "execute",
            "status": status,
        });
        update[if kind == "tool_call" || status == "in_progress" {
            "rawInput"
        } else {
            "rawOutput"
        }] = value;
        serde_json::from_value(update).unwrap()
    }
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let make_state = || {
        let observed = contexts.clone();
        FoldState::with_workspace(
            None,
            WorkspaceContext {
                cwd: Some("/repo/worktrees/pr-153".to_string()),
                branch: Some("fix/pr-context".to_string()),
                pull_request: None,
            },
            false,
            Some(Box::new(move |context| {
                observed.lock().unwrap().push(context)
            })),
        )
    };
    let call = || {
        update(
            "tool_call",
            "in_progress",
            serde_json::json!({"command": "cd /repo/worktrees/pr-153 && gh pr view --json url"}),
        )
    };
    let result = |status| {
        update(
            "tool_call_update",
            status,
            serde_json::json!("{\"url\":\"https://github.com/acme/repo/pull/153\"}"),
        )
    };

    let mut completed = make_state();
    completed.fold(call());
    completed.fold(result("completed"));
    assert_eq!(contexts.lock().unwrap().len(), 1);

    contexts.lock().unwrap().clear();
    let mut failed = make_state();
    failed.fold(call());
    failed.fold(result("failed"));
    assert!(contexts.lock().unwrap().is_empty());

    let mut moved = make_state();
    moved.fold(call());
    moved.workspace_context.branch = Some("another-branch".to_string());
    moved.fold(result("completed"));
    assert!(contexts.lock().unwrap().is_empty());

    let mut replaced = make_state();
    replaced.fold(call());
    replaced.fold(update(
        "tool_call_update",
        "in_progress",
        serde_json::json!({"command": "gh pr create --head another-branch"}),
    ));
    replaced.fold(result("completed"));
    assert!(contexts.lock().unwrap().is_empty());

    let mut final_replaced = make_state();
    final_replaced.fold(call());
    let terminal_replacement = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": "call-pr",
        "status": "completed",
        "rawInput": {"command": "gh pr create --head another-branch"},
        "rawOutput": "{\"url\":\"https://github.com/acme/repo/pull/153\"}"
    }))
    .unwrap();
    final_replaced.fold(terminal_replacement);
    assert!(contexts.lock().unwrap().is_empty());
}
