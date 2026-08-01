//! Tests for the loopback workflow host and the per-turn copilot dispatch.

use super::*;
use crate::hub::{RunError, TaskRequest};

/// One authoring turn, as the copilot builds it.
fn request() -> TaskRequest {
    TaskRequest {
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
