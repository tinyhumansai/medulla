//! Tests for the loopback workflow host and the per-turn copilot dispatch.

use super::*;
use crate::hub::{RunError, TaskRequest};

#[cfg(unix)]
struct NoFleet;

#[cfg(unix)]
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
        _request: TaskRequest,
        _status: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<crate::hub::TaskOutcome, RunError> {
        unreachable!("grant preparation never dispatches")
    }

    fn abort(&self, _abort_id: &str) -> bool {
        false
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_workflow_run_converts_its_parent_grant_into_a_verified_handoff() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("control.sock");
    let grants = crate::control_socket::GrantRegistry::new();
    let token = grants.mint(crate::control_socket::Grant::new("parent", 1, 3));
    let ops: Arc<dyn crate::control_socket::FleetOps> = Arc::new(NoFleet);
    let _server = crate::control_socket::ControlServer::bind(&path, ops, grants)
        .await
        .unwrap();
    let env = std::collections::HashMap::from([
        (
            crate::control_socket::MCP_SOCKET_ENV.to_string(),
            path.to_string_lossy().into_owned(),
        ),
        (
            crate::control_socket::MCP_GRANT_ENV.to_string(),
            token.clone(),
        ),
    ]);

    let (nested, depth) = nested_harness_env(&env).await.unwrap();

    assert_eq!(depth, 2);
    assert!(!nested.contains_key(crate::control_socket::MCP_SOCKET_ENV));
    assert!(!nested.contains_key(crate::control_socket::MCP_GRANT_ENV));
    assert_eq!(
        nested.get(crate::control_socket::MCP_PARENT_GRANT_ENV),
        Some(&token)
    );
}

/// One authoring turn, as the copilot builds it.
fn request() -> TaskRequest {
    TaskRequest {
        transport: None,
        task_id: "copilot-1".into(),
        abort_id: "copilot-1".into(),
        cycle_id: None,
        instruction: "add a slack step".into(),
        worker_address: LOCAL_WORKER_ADDRESS.into(),
        provider: None,
        custom_harness: None,
        model: None,
        // An authoring turn gets the full workflow tool surface; only a review
        // pass narrows it.
        tool_mode: None,
        workflow: None,
        workflow_fingerprint: None,
        workflow_inputs: Default::default(),
        // The copilot keys its turns to a conversation, so a fixture standing in
        // for one carries a key too.
        conversation: Some("cloud-copilot:demo".into()),
        fleet_depth: 0,
    }
}

#[tokio::test]
async fn a_turn_with_no_agent_installed_fails_instead_of_hanging() {
    // The failure the backend must never wait ten minutes for: this host cannot
    // start a harness at all. Requesting an empty provider set reproduces it
    // without depending on what is on this machine's PATH.
    let dispatch = LocalCopilotDispatch::new(EmbeddedDaemonOptions {
        providers: Some(Vec::new()),
        ..Default::default()
    });

    let error = dispatch.dispatch(request()).await.expect_err("no harness");

    match error {
        RunError::Transport(message) => assert!(
            message.contains("none of the requested coding-agent CLIs are installed"),
            "{message}"
        ),
        other => panic!("expected a transport failure, got {other:?}"),
    }
}
