//! Provider-level regressions for pull-request command/result correlation.

use serde_json::json;

use crate::harness_work::kinds;

use super::HarnessLineMapper;

/// Map a provider transcript through one stateful mapper.
fn map(provider: &str, lines: &[serde_json::Value]) -> Vec<super::HarnessSemanticEvent> {
    let mut mapper = HarnessLineMapper::new_with_gh_repo_override(provider, false);
    lines
        .iter()
        .enumerate()
        .flat_map(|(line, value)| mapper.map_line(&value.to_string(), line as i64))
        .collect()
}

#[test]
fn claude_failed_pr_commands_do_not_publish_a_url() {
    let events = map(
        "claude",
        &[
            json!({
                "type": "assistant",
                "message": {"role": "assistant", "content": [{
                    "type": "tool_use", "id": "pr-1", "name": "Bash",
                    "input": {"command": "gh pr create --fill"}
                }]}
            }),
            json!({
                "type": "user",
                "message": {"role": "user", "content": [{
                    "type": "tool_result", "tool_use_id": "pr-1", "is_error": true,
                    "content": "hook failed: https://github.com/acme/repo/pull/42"
                }]}
            }),
        ],
    );
    assert!(!events
        .iter()
        .any(|event| event.event.kind == kinds::SESSION_INFO));
}

#[test]
fn codex_failed_pr_commands_do_not_publish_a_url() {
    let command = "/bin/zsh -lc 'gh pr create --fill'";
    let events = map(
        "codex",
        &[
            json!({
                "type": "item.started",
                "item": {"id": "pr-1", "type": "command_execution", "command": command}
            }),
            json!({
                "type": "item.completed",
                "item": {
                    "id": "pr-1", "type": "command_execution", "command": command,
                    "aggregated_output": "hook failed: https://github.com/acme/repo/pull/42",
                    "exit_code": 1
                }
            }),
        ],
    );
    assert!(!events
        .iter()
        .any(|event| event.event.kind == kinds::SESSION_INFO));
}
