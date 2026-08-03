//! ACP notification folding and workspace-correlation regressions.

use std::sync::{Arc, Mutex};

use crate::daemon::status_detail;
use crate::sessions::WorkspaceContext;

use super::super::types::FoldState;

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
