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

    let mut moved_with_repeated_input = make_state();
    moved_with_repeated_input.fold(call());
    moved_with_repeated_input.workspace_context = WorkspaceContext {
        cwd: Some("/repo/worktrees/another-pr".to_string()),
        branch: Some("another-branch".to_string()),
        pull_request: None,
    };
    let repeated_terminal = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": "call-pr",
        "kind": "execute",
        "status": "completed",
        "rawInput": {
            "command": "cd /repo/worktrees/pr-153 && gh pr view --json url"
        },
        "rawOutput": "{\"url\":\"https://github.com/acme/repo/pull/153\"}"
    }))
    .unwrap();
    moved_with_repeated_input.fold(repeated_terminal);
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

    let mut non_execute = make_state();
    non_execute.fold(update(
        "tool_call",
        "in_progress",
        serde_json::json!({"command": "gh pr view --json url"}),
    ));
    let kind_replacement = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": "call-pr",
        "kind": "read",
        "status": "in_progress"
    }))
    .unwrap();
    non_execute.fold(kind_replacement);
    non_execute.fold(result("completed"));
    assert!(contexts.lock().unwrap().is_empty());

    let read_call: agent_client_protocol::schema::v1::SessionUpdate =
        serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "call-pr",
        "title": "Read",
        "kind": "read",
        "status": "in_progress",
        "rawInput": {"command": "cd /repo/worktrees/pr-153 && gh pr view --json url"}
        }))
        .unwrap();
    let mut read_only = make_state();
    read_only.fold(read_call.clone());
    read_only.fold(result("completed"));
    assert!(contexts.lock().unwrap().is_empty());

    let mut becomes_execute = make_state();
    becomes_execute.fold(read_call);
    let execute_update = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": "call-pr",
        "kind": "execute",
        "status": "in_progress"
    }))
    .unwrap();
    becomes_execute.fold(execute_update);
    becomes_execute.fold(result("completed"));
    assert_eq!(contexts.lock().unwrap().len(), 1);
}

#[test]
fn a_worktree_report_moves_the_directory_local_hooks_run_in() {
    // The session starts in the daemon's checkout and continues in a worktree
    // it creates mid-session. `workspace_cwd` is what the Codex ACP PostToolUse
    // fallback reads, so it has to follow that move — otherwise an auto-commit
    // hook checkpoints the launch checkout after every later edit.
    let mut state = FoldState::new(None);
    assert_eq!(state.workspace_cwd(), None);

    let call = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call",
        "toolCallId": "call-worktree",
        "title": "Terminal",
        "kind": "execute",
        "status": "in_progress",
        "rawInput": { "command": "worktree hook-cwd" }
    }))
    .unwrap();
    let report = serde_json::from_value(serde_json::json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": "call-worktree",
        "status": "completed",
        "rawOutput": "[PASS] WORKTREE_READY\n  path: /repo/worktrees/hook-cwd\n  branch: hook-cwd\n"
    }))
    .unwrap();
    state.fold(call);
    let completed = state.fold(report);

    assert!(
        completed.is_some(),
        "the report's own tool call still completes, so its hooks run too"
    );
    assert_eq!(state.workspace_cwd(), Some("/repo/worktrees/hook-cwd"));
}

#[test]
fn a_resumed_session_starts_from_the_directory_its_last_turn_ended_in() {
    // Workspace context is persisted across turns; a resumed turn must not
    // rediscover the worktree before its hooks are aimed at the right one.
    let state = FoldState::with_workspace(
        None,
        WorkspaceContext {
            cwd: Some("/repo/worktrees/hook-cwd".to_string()),
            branch: Some("hook-cwd".to_string()),
            pull_request: None,
        },
        false,
        None,
    );
    assert_eq!(state.workspace_cwd(), Some("/repo/worktrees/hook-cwd"));
}

/// The stream `codex-acp` 1.7.0 produces when the provider answers a turn with
/// an HTTP error: a `willRetry` error per attempt, a bare `systemError` thread
/// status when it gives up, and then the provider's complaint streamed as if it
/// were the assistant's answer. Captured from a real 402 run.
fn codex_failure_updates() -> Vec<serde_json::Value> {
    let detail = "unexpected status 402 Payment Required: Insufficient credits. \
                  Add more using https://openrouter.ai/settings/credits";
    let mut updates: Vec<serde_json::Value> = (1..=5)
        .map(|attempt| {
            serde_json::json!({
                "sessionUpdate": "session_info_update",
                "_meta": {"codex": {"error": {
                    "message": format!("Reconnecting... {attempt}/5"),
                    "additionalDetails": detail,
                    "willRetry": true,
                }}}
            })
        })
        .collect();
    updates.push(serde_json::json!({
        "sessionUpdate": "session_info_update",
        "_meta": {"codex": {"threadStatus": {"type": "systemError"}}}
    }));
    updates.push(serde_json::json!({
        "sessionUpdate": "agent_message_chunk",
        "content": {"type": "text", "text": detail}
    }));
    updates
}

#[test]
fn a_codex_turn_that_gave_up_retrying_is_recorded_as_failed() {
    let mut state = FoldState::new(None);
    for update in codex_failure_updates() {
        state.fold(serde_json::from_value(update).unwrap());
    }
    // Without this the run answers `stopReason: end_turn` and the provider's
    // 402 becomes the agent's reply — a workflow step marked success whose
    // output is an error message.
    assert_eq!(
        state.error.as_deref(),
        Some(
            "unexpected status 402 Payment Required: Insufficient credits. \
             Add more using https://openrouter.ai/settings/credits"
        )
    );
}

#[test]
fn a_codex_error_still_being_retried_does_not_fail_the_turn() {
    let mut state = FoldState::new(None);
    for update in codex_failure_updates().into_iter().take(3) {
        state.fold(serde_json::from_value(update).unwrap());
    }
    assert_eq!(state.error, None);
}

#[test]
fn a_codex_error_that_will_not_be_retried_fails_the_turn_on_its_own() {
    let mut state = FoldState::new(None);
    let update = serde_json::json!({
        "sessionUpdate": "session_info_update",
        "_meta": {"codex": {"error": {
            "message": "stream disconnected",
            "willRetry": false,
        }}}
    });
    state.fold(serde_json::from_value(update).unwrap());
    assert_eq!(state.error.as_deref(), Some("stream disconnected"));
}

#[test]
fn an_ordinary_session_info_update_is_not_a_failure() {
    let mut state = FoldState::new(None);
    for update in [
        serde_json::json!({"sessionUpdate": "session_info_update", "title": "Reply with PONG"}),
        serde_json::json!({
            "sessionUpdate": "session_info_update",
            "_meta": {"codex": {"threadStatus": {"type": "active", "activeFlags": []}}}
        }),
    ] {
        state.fold(serde_json::from_value(update).unwrap());
    }
    assert_eq!(state.error, None);
}
