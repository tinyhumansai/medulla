//! ACP terminal-call correlation tests for retained repository context.

use std::sync::{Arc, Mutex};

use crate::sessions::WorkspaceContext;

use super::types::FoldState;

fn update(
    kind: &str,
    status: &str,
    command_or_output: serde_json::Value,
) -> agent_client_protocol::schema::v1::SessionUpdate {
    let mut value = serde_json::json!({
        "sessionUpdate": kind,
        "toolCallId": "call-pr",
        "title": "Terminal",
        "kind": "execute",
        "status": status,
    });
    let field = if kind == "tool_call" {
        "rawInput"
    } else {
        "rawOutput"
    };
    value[field] = command_or_output;
    serde_json::from_value(value).unwrap()
}

fn state(contexts: Arc<Mutex<Vec<WorkspaceContext>>>) -> FoldState {
    FoldState::with_workspace(
        None,
        WorkspaceContext {
            cwd: Some("/repo/worktrees/pr-153".to_string()),
            branch: Some("fix/pr-context".to_string()),
            pull_request: None,
        },
        false,
        Some(Box::new(move |context| {
            contexts.lock().unwrap().push(context)
        })),
    )
}

#[test]
fn completed_pr_results_persist_workspace_context() {
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let mut state = state(contexts.clone());
    state.fold(update(
        "tool_call",
        "in_progress",
        serde_json::json!({"command": "cd /repo/worktrees/pr-153 && gh pr view --json url"}),
    ));
    state.fold(update(
        "tool_call_update",
        "completed",
        serde_json::json!("{\"url\":\"https://github.com/acme/repo/pull/153\"}"),
    ));

    assert_eq!(
        contexts
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .pull_request
            .as_deref(),
        Some("https://github.com/acme/repo/pull/153")
    );
}

#[test]
fn failed_pr_results_are_not_authoritative() {
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let mut state = state(contexts.clone());
    state.fold(update(
        "tool_call",
        "in_progress",
        serde_json::json!({"command": "cd /repo/worktrees/pr-153 && gh pr view --json url"}),
    ));
    state.fold(update(
        "tool_call_update",
        "failed",
        serde_json::json!("{\"url\":\"https://github.com/acme/repo/pull/999\"}"),
    ));
    assert!(contexts.lock().unwrap().is_empty());
}

#[test]
fn pr_results_are_rejected_after_the_workspace_changes() {
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let mut state = state(contexts.clone());
    state.fold(update(
        "tool_call",
        "in_progress",
        serde_json::json!({"command": "cd /repo/worktrees/pr-153 && gh pr view --json url"}),
    ));
    state.workspace_context.branch = Some("another-branch".to_string());
    state.fold(update(
        "tool_call_update",
        "completed",
        serde_json::json!("{\"url\":\"https://github.com/acme/repo/pull/153\"}"),
    ));
    assert!(contexts.lock().unwrap().is_empty());
}
