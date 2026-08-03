//! The codex item fold: one `item.started` / `item.completed` record into
//! semantic events.
//!
//! Modern `codex exec --json` reports everything it does as a typed *item* —
//! the reply, its reasoning, each command, the todo list, the files it wrote,
//! every MCP call and web search. Only the first three were folded before, so a
//! codex worker's plan, edits, and tool use existed on the wire and never
//! reached a screen. This module folds the rest.

use serde_json::Value;
use std::collections::HashMap;

use crate::harness_work::kinds;

use super::super::events::{semantic, tool_call_payload, tool_result_payload};
use super::super::shared::text_from_content;
use super::super::types::HarnessSemanticEvent;
use super::super::workspace::{
    pull_request_command, workspace_event_from_output, PendingPullRequestCall,
};

/// Map a codex `item.started`/`item.completed` record to semantic events.
///
/// `agent_message` is the reply (emitted once, on completion); `reasoning` is
/// chain-of-thought; `command_execution` becomes a `tool_call` when it starts
/// and a `tool_result` when it ends; `todo_list` and `file_change` become work
/// events; `mcp_tool_call` and `web_search` become tool calls and results; an
/// `error` item becomes an error. Genuinely unknown item kinds are still ignored
/// rather than guessed at.
pub(super) fn codex_item(
    record: &serde_json::Map<String, Value>,
    record_type: &str,
    ts: i64,
    line: i64,
    pull_request_calls: &mut HashMap<String, PendingPullRequestCall>,
    workspace: (Option<&str>, Option<&str>),
    gh_repo_is_set: bool,
) -> Vec<HarnessSemanticEvent> {
    let item = match record.get("item").and_then(Value::as_object) {
        Some(item) => item,
        None => return Vec::new(),
    };
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
    let id = item.get("id").and_then(Value::as_str).unwrap_or("");
    let completed = record_type == "item.completed";
    let tag = |suffix: &str| format!("{record_type}:{suffix}");

    match item_type {
        "agent_message" if completed => text_event(
            item,
            "text",
            line,
            ts,
            &tag("agent_message"),
            "agent_message",
        ),
        "reasoning" if completed => {
            let text = item
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| {
                    text_from_content(item.get("summary"), &["summary_text", "text"])
                });
            if text.is_empty() {
                return Vec::new();
            }
            vec![semantic(
                line,
                ts,
                &tag("reasoning"),
                "agent_thinking",
                "agent",
                serde_json::json!({ "text": text }),
            )]
        }
        "command_execution" => command_event(
            item,
            id,
            completed,
            (line, ts),
            &tag("command_execution"),
            pull_request_calls,
            (workspace.0, workspace.1, gh_repo_is_set),
        ),
        // Codex's own todo list, the closest thing it has to Claude's TodoWrite.
        // Emitted on both started and completed so a list that is written once
        // and then ticked off still moves on screen.
        "todo_list" => todo_event(item, line, ts, &tag("todo_list")),
        "file_change" => file_change_events(item, line, ts, &tag("file_change")),
        "mcp_tool_call" => mcp_events(item, id, completed, line, ts, &tag("mcp_tool_call")),
        "web_search" => web_search_events(item, id, completed, line, ts, &tag("web_search")),
        "error" => {
            let message = ["message", "error", "text"]
                .iter()
                .find_map(|key| item.get(*key).and_then(Value::as_str))
                .unwrap_or("");
            if message.is_empty() {
                return Vec::new();
            }
            vec![semantic(
                line,
                ts,
                &tag("error"),
                "error",
                "agent",
                serde_json::json!({ "message": message, "fatal": false }),
            )]
        }
        _ => Vec::new(),
    }
}

/// A one-field text event, or nothing when that field is empty.
fn text_event(
    item: &serde_json::Map<String, Value>,
    key: &str,
    line: i64,
    ts: i64,
    record_type: &str,
    kind: &str,
) -> Vec<HarnessSemanticEvent> {
    let text = item.get(key).and_then(Value::as_str).unwrap_or("");
    if text.is_empty() {
        return Vec::new();
    }
    vec![semantic(
        line,
        ts,
        record_type,
        kind,
        "agent",
        serde_json::json!({ "text": text }),
    )]
}

/// A shell command as a call on start and a result on completion.
fn command_event(
    item: &serde_json::Map<String, Value>,
    id: &str,
    completed: bool,
    position: (i64, i64),
    record_type: &str,
    pull_request_calls: &mut HashMap<String, PendingPullRequestCall>,
    workspace: (Option<&str>, Option<&str>, bool),
) -> Vec<HarnessSemanticEvent> {
    let (line, ts) = position;
    let (workspace_cwd, workspace_branch, gh_repo_is_set) = workspace;
    if !completed {
        let command = item.get("command").and_then(Value::as_str).unwrap_or("");
        if !id.is_empty() {
            if let Some(command) = pull_request_command(command, workspace_cwd, gh_repo_is_set) {
                pull_request_calls.insert(
                    id.to_string(),
                    PendingPullRequestCall::new(command, workspace_cwd, workspace_branch),
                );
            }
        }
        return vec![semantic(
            line,
            ts,
            record_type,
            "tool_call",
            "agent",
            tool_call_payload(id, "shell", &Value::String(command.to_string())),
        )];
    }
    let output = item
        .get("aggregated_output")
        .and_then(Value::as_str)
        .unwrap_or("");
    let is_error = item
        .get("exit_code")
        .and_then(Value::as_i64)
        .map(|code| code != 0)
        .unwrap_or(false);
    let mut events = vec![semantic(
        line,
        ts,
        record_type,
        "tool_result",
        "agent",
        tool_result_payload(id, is_error, output),
    )];
    events.extend(workspace_event_from_output(
        output,
        pull_request_calls
            .remove(id)
            .and_then(|call| call.command_in(workspace_cwd, workspace_branch)),
        line,
        ts,
        &format!("{record_type}:workspace"),
    ));
    events
}

/// Codex's todo list as a `todo_update`.
///
/// Its items are `{ text, completed }` rather than Claude's
/// `{ content, status }`; the shared work fold reads both, so they are passed
/// through as-is instead of being rewritten here.
fn todo_event(
    item: &serde_json::Map<String, Value>,
    line: i64,
    ts: i64,
    record_type: &str,
) -> Vec<HarnessSemanticEvent> {
    let items = ["items", "todos", "plan"]
        .iter()
        .find_map(|key| item.get(*key))
        .and_then(Value::as_array);
    let Some(items) = items.filter(|items| !items.is_empty()) else {
        return Vec::new();
    };
    vec![semantic(
        line,
        ts,
        record_type,
        kinds::TODO_UPDATE,
        "agent",
        serde_json::json!({ "todos": items, "source": "codex:todo_list" }),
    )]
}

/// One `file_change` event per path in a codex file-change item.
fn file_change_events(
    item: &serde_json::Map<String, Value>,
    line: i64,
    ts: i64,
    record_type: &str,
) -> Vec<HarnessSemanticEvent> {
    let changes = ["changes", "files"]
        .iter()
        .find_map(|key| item.get(*key))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    changes
        .iter()
        .filter_map(|change| {
            let object = change.as_object()?;
            let path = ["path", "file", "file_path"]
                .iter()
                .find_map(|key| object.get(*key))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|path| !path.is_empty())?;
            let kind = ["kind", "change", "type"]
                .iter()
                .find_map(|key| object.get(*key))
                .and_then(Value::as_str)
                .unwrap_or("modified");
            Some(semantic(
                line,
                ts,
                record_type,
                kinds::FILE_CHANGE,
                "agent",
                serde_json::json!({
                    "path": path,
                    "change": kind,
                    "tool": "codex:file_change",
                }),
            ))
        })
        .collect()
}

/// An MCP call as a call on start and a result on completion.
fn mcp_events(
    item: &serde_json::Map<String, Value>,
    id: &str,
    completed: bool,
    line: i64,
    ts: i64,
    record_type: &str,
) -> Vec<HarnessSemanticEvent> {
    let server = item.get("server").and_then(Value::as_str).unwrap_or("");
    let tool = item
        .get("tool")
        .and_then(Value::as_str)
        .or_else(|| item.get("name").and_then(Value::as_str))
        .unwrap_or("mcp");
    let name = if server.is_empty() {
        tool.to_string()
    } else {
        format!("{server}__{tool}")
    };
    if !completed {
        let input = item
            .get("arguments")
            .or_else(|| item.get("input"))
            .cloned()
            .unwrap_or(Value::Null);
        let mut payload = tool_call_payload(id, &name, &input);
        payload["tool_kind"] = Value::String("mcp".to_string());
        return vec![semantic(
            line,
            ts,
            record_type,
            "tool_call",
            "agent",
            payload,
        )];
    }
    let output = item
        .get("result")
        .or_else(|| item.get("output"))
        .map(|value| match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        })
        .unwrap_or_default();
    let is_error = item.get("status").and_then(Value::as_str) == Some("failed")
        || item.get("success") == Some(&Value::Bool(false));
    vec![semantic(
        line,
        ts,
        record_type,
        "tool_result",
        "agent",
        tool_result_payload(id, is_error, &output),
    )]
}

/// A web search as a call on start and a result on completion.
fn web_search_events(
    item: &serde_json::Map<String, Value>,
    id: &str,
    completed: bool,
    line: i64,
    ts: i64,
    record_type: &str,
) -> Vec<HarnessSemanticEvent> {
    let query = item
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !completed {
        return vec![semantic(
            line,
            ts,
            record_type,
            "tool_call",
            "agent",
            tool_call_payload(id, "web_search", &Value::String(query)),
        )];
    }
    // The completed item repeats the query rather than carrying results; report
    // the search as done rather than inventing an output it never sent.
    vec![semantic(
        line,
        ts,
        record_type,
        "tool_result",
        "agent",
        tool_result_payload(id, false, &query),
    )]
}
