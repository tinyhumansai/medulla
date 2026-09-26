//! Provider-level tests for associating GitHub CLI results with a session.

use serde_json::json;

use medulla::daemon::mappers::HarnessLineMapper;
use medulla::harness_work::WorkFold;

/// Map a transcript and fold its semantic events into session state.
fn snapshot(provider: &str, lines: &[serde_json::Value]) -> medulla::harness_work::WorkSnapshot {
    let mut mapper = HarnessLineMapper::new_with_gh_repo_override(provider, false);
    let mut fold = WorkFold::new();
    for (line, value) in lines.iter().enumerate() {
        for event in mapper.map_line(&value.to_string(), line as i64) {
            fold.apply(&event.event.kind, &event.event.payload, event.timestamp_ms);
        }
    }
    fold.into_snapshot()
}

/// A Claude assistant record wrapping one shell call.
fn claude_shell(id: &str, command: &str) -> serde_json::Value {
    json!({
        "type": "assistant",
        "message": { "role": "assistant", "content": [{
            "type": "tool_use", "id": id, "name": "Bash",
            "input": { "command": command }
        }]}
    })
}

/// A Claude user record returning one shell call's output.
fn claude_result(id: &str, output: &str) -> serde_json::Value {
    json!({
        "type": "user",
        "message": { "role": "user", "content": [{
            "type": "tool_result", "tool_use_id": id, "content": output
        }]}
    })
}

#[test]
fn claude_pr_output_attaches_the_review_to_the_session() {
    let result = snapshot(
        "claude",
        &[
            claude_shell("pr-1", "gh pr create --title fix"),
            claude_result("pr-1", "https://github.com/tinyhumansai/medulla/pull/157"),
        ],
    );
    assert_eq!(
        result.info.pull_request.as_deref(),
        Some("https://github.com/tinyhumansai/medulla/pull/157")
    );
}

#[test]
fn claude_unrelated_output_cannot_replace_the_sessions_pull_request() {
    let result = snapshot(
        "claude",
        &[
            claude_shell("search-1", "rg pull fixtures"),
            claude_result("search-1", "https://github.com/tinyhumansai/other/pull/999"),
        ],
    );
    assert!(result.info.pull_request.is_none());
}

#[test]
fn codex_pr_json_attaches_the_review_to_the_session() {
    let result = snapshot(
        "codex",
        &[
            json!({
                "type": "item.started",
                "item": {
                    "type": "command_execution", "id": "cmd-pr",
                    "command": "gh pr view --json url"
                }
            }),
            json!({
                "type": "item.completed",
                "item": {
                    "type": "command_execution", "id": "cmd-pr",
                    "command": "gh pr view --json url",
                    "aggregated_output": "{\"url\":\"https://github.com/tinyhumansai/medulla/pull/158\"}",
                    "exit_code": 0
                }
            }),
        ],
    );
    assert_eq!(
        result.info.pull_request.as_deref(),
        Some("https://github.com/tinyhumansai/medulla/pull/158")
    );
}

#[test]
fn modern_codex_pr_result_is_rejected_after_workspace_switch() {
    let result = snapshot(
        "codex",
        &[
            json!({
                "type": "item.started",
                "item": {
                    "type": "command_execution", "id": "pr-before-switch",
                    "command": "gh pr create --fill"
                }
            }),
            json!({
                "type": "item.completed",
                "item": {
                    "type": "command_execution", "id": "worktree-switch",
                    "command": "worktree switched",
                    "aggregated_output": "[PASS] WORKTREE_READY\n  path: /repo/worktrees/switched\n  branch: switched\n",
                    "exit_code": 0
                }
            }),
            json!({
                "type": "item.completed",
                "item": {
                    "type": "command_execution", "id": "pr-before-switch",
                    "command": "gh pr create --fill",
                    "aggregated_output": "https://github.com/tinyhumansai/medulla/pull/164",
                    "exit_code": 0
                }
            }),
        ],
    );
    assert_eq!(result.info.cwd.as_deref(), Some("/repo/worktrees/switched"));
    assert!(result.info.pull_request.is_none());
}

#[test]
fn codex_unrelated_output_cannot_replace_the_sessions_pull_request() {
    let result = snapshot(
        "codex",
        &[json!({
            "type": "item.completed",
            "item": {
                "type": "command_execution", "id": "cmd-search",
                "command": "rg pull fixtures",
                "aggregated_output": "https://github.com/tinyhumansai/other/pull/999",
                "exit_code": 0
            }
        })],
    );
    assert!(result.info.pull_request.is_none());
}

#[test]
fn legacy_codex_pr_output_attaches_the_review_by_call_id() {
    let result = snapshot(
        "codex",
        &[
            json!({
                "type": "response_item",
                "payload": {
                    "type": "function_call", "name": "shell", "call_id": "pr-legacy",
                    "arguments": "{\"command\":\"gh pr view --json url\"}"
                }
            }),
            json!({
                "type": "response_item",
                "payload": {
                    "type": "function_call_output", "call_id": "pr-legacy",
                    "output": "{\"url\":\"https://github.com/tinyhumansai/medulla/pull/159\"}"
                }
            }),
        ],
    );
    assert_eq!(
        result.info.pull_request.as_deref(),
        Some("https://github.com/tinyhumansai/medulla/pull/159")
    );
}

#[test]
fn legacy_codex_unrelated_output_cannot_replace_the_sessions_pull_request() {
    let result = snapshot(
        "codex",
        &[
            json!({
                "type": "response_item",
                "payload": {
                    "type": "function_call", "name": "shell", "call_id": "search-legacy",
                    "arguments": "{\"command\":\"rg pull fixtures\"}"
                }
            }),
            json!({
                "type": "response_item",
                "payload": {
                    "type": "function_call_output", "call_id": "search-legacy",
                    "output": "https://github.com/tinyhumansai/other/pull/999"
                }
            }),
        ],
    );
    assert!(result.info.pull_request.is_none());
}

#[test]
fn claude_pr_from_reported_worktree_attaches_the_review() {
    let result = snapshot(
        "claude",
        &[
            claude_shell("worktree-1", "worktree fix-label"),
            claude_result(
                "worktree-1",
                "[PASS] WORKTREE_READY\n  path: /repo/worktrees/fix-label\n  branch: fix-label\n",
            ),
            claude_shell(
                "pr-worktree",
                "cd /repo/worktrees/fix-label && gh pr create --fill",
            ),
            claude_result(
                "pr-worktree",
                "https://github.com/tinyhumansai/medulla/pull/160",
            ),
        ],
    );
    assert_eq!(
        result.info.pull_request.as_deref(),
        Some("https://github.com/tinyhumansai/medulla/pull/160")
    );
}

#[test]
fn claude_bare_pr_after_worktree_report_is_not_attached() {
    let result = snapshot(
        "claude",
        &[
            claude_shell("worktree-1", "worktree fix-label"),
            claude_result(
                "worktree-1",
                "[PASS] WORKTREE_READY\n  path: /repo/worktrees/fix-label\n  branch: fix-label\n",
            ),
            claude_shell("pr-launch-checkout", "gh pr create --fill"),
            claude_result(
                "pr-launch-checkout",
                "https://github.com/tinyhumansai/medulla/pull/161",
            ),
        ],
    );
    assert!(result.info.pull_request.is_none());
}

#[test]
fn claude_batched_worktree_results_track_the_last_checkout() {
    let result = snapshot(
        "claude",
        &[
            claude_shell("worktree-1", "worktree first"),
            claude_shell("worktree-2", "worktree second"),
            json!({
                "type": "user",
                "message": { "role": "user", "content": [
                    {
                        "type": "tool_result", "tool_use_id": "worktree-1",
                        "content": "[PASS] WORKTREE_READY\n  path: /repo/worktrees/first\n  branch: first\n"
                    },
                    {
                        "type": "tool_result", "tool_use_id": "worktree-2",
                        "content": "[PASS] WORKTREE_READY\n  path: /repo/worktrees/second\n  branch: second\n"
                    }
                ]}
            }),
            claude_shell(
                "pr-second",
                "cd /repo/worktrees/second && gh pr create --fill",
            ),
            claude_result(
                "pr-second",
                "https://github.com/tinyhumansai/medulla/pull/162",
            ),
        ],
    );
    assert_eq!(
        result.info.pull_request.as_deref(),
        Some("https://github.com/tinyhumansai/medulla/pull/162")
    );
}

#[test]
fn claude_pr_result_is_rejected_after_batched_workspace_switch() {
    let result = snapshot(
        "claude",
        &[
            claude_shell("worktree-a", "worktree workspace-a"),
            claude_result(
                "worktree-a",
                "[PASS] WORKTREE_READY\n  path: /repo/worktrees/workspace-a\n  branch: workspace-a\n",
            ),
            claude_shell(
                "pr-a",
                "cd /repo/worktrees/workspace-a && gh pr create --fill",
            ),
            claude_shell("worktree-b", "worktree workspace-b"),
            json!({
                "type": "user",
                "message": { "role": "user", "content": [
                    {
                        "type": "tool_result", "tool_use_id": "worktree-b",
                        "content": "[PASS] WORKTREE_READY\n  path: /repo/worktrees/workspace-a\n  branch: workspace-b\n"
                    },
                    {
                        "type": "tool_result", "tool_use_id": "pr-a",
                        "content": "https://github.com/tinyhumansai/medulla/pull/163"
                    }
                ]}
            }),
        ],
    );
    assert_eq!(
        result.info.cwd.as_deref(),
        Some("/repo/worktrees/workspace-a")
    );
    assert_eq!(result.info.branch.as_deref(), Some("workspace-b"));
    assert!(result.info.pull_request.is_none());
}
