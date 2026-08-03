//! Claude flat-run mapper (`claude -p --output-format stream-json`): fold a user
//! or assistant transcript record into user_prompt, agent_message,
//! agent_thinking, tool_call, and tool_result semantic events, plus the work
//! events its `system`, `result`, and structured tool records imply.

use serde_json::Value;
use std::collections::HashMap;

use crate::harness_work::kinds;

use super::events::{semantic, tool_call_payload, tool_result_payload, user_prompt_event};
use super::shared::{as_array, parse_json_object};
use super::timestamp::parse_timestamp_ms;
use super::types::HarnessSemanticEvent;
use super::work::work_events_for_tool;
use super::workspace::{pull_request_command, workspace_event_from_output, PullRequestCommand};

/// Map one raw Claude JSONL line into zero or more semantic events.
pub(super) fn claude_events_from_line(
    raw: &str,
    line: i64,
    pull_request_calls: &mut HashMap<String, PullRequestCommand>,
) -> Vec<HarnessSemanticEvent> {
    let record = match parse_json_object(raw) {
        Some(record) => record,
        None => return Vec::new(),
    };
    let ts = parse_timestamp_ms(record.get("timestamp"));

    // The two envelope records carry no `message`: the init banner describing
    // the session, and the terminal result. Both were dropped before, which is
    // why a run's model, working directory, cost, and outcome never reached the
    // screen even though the harness announced all four.
    match record.get("type").and_then(Value::as_str) {
        Some("system") => return claude_system(&record, line, ts),
        Some("result") => return claude_result(&record, line, ts),
        _ => {}
    }

    let message = match record.get("message").and_then(Value::as_object) {
        Some(message) => message,
        None => return Vec::new(),
    };
    let source_role = message.get("role").and_then(Value::as_str);
    let record_type = record.get("type").and_then(Value::as_str);

    if record_type == Some("user") && source_role == Some("user") {
        if let Some(text) = message.get("content").and_then(Value::as_str) {
            return if text.is_empty() {
                Vec::new()
            } else {
                vec![user_prompt_event(line, ts, text)]
            };
        }
        return as_array(message.get("content"))
            .iter()
            .flat_map(|block| claude_user_block(block, line, ts, pull_request_calls))
            .collect();
    }

    if record_type == Some("assistant") && source_role == Some("assistant") {
        return as_array(message.get("content"))
            .iter()
            .flat_map(|block| claude_assistant_block(block, line, ts, pull_request_calls))
            .collect();
    }

    Vec::new()
}

/// Fold a single block of a user message (text prompt or tool_result).
fn claude_user_block(
    block: &Value,
    line: i64,
    ts: i64,
    pull_request_calls: &mut HashMap<String, PullRequestCommand>,
) -> Vec<HarnessSemanticEvent> {
    let object = match block.as_object() {
        Some(object) => object,
        None => return Vec::new(),
    };
    match object.get("type").and_then(Value::as_str) {
        Some("text") => {
            let text = object.get("text").and_then(Value::as_str).unwrap_or("");
            if text.is_empty() {
                Vec::new()
            } else {
                vec![user_prompt_event(line, ts, text)]
            }
        }
        Some("tool_result") => {
            let call_id = object
                .get("tool_use_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            let is_error = object.get("is_error") == Some(&Value::Bool(true));
            let output = flatten_claude_tool_result(object.get("content"));
            let mut events = vec![semantic(
                line,
                ts,
                "user:tool_result",
                "tool_result",
                "agent",
                tool_result_payload(call_id, is_error, &output),
            )];
            events.extend(workspace_event_from_output(
                &output,
                pull_request_calls.remove(call_id),
                line,
                ts,
                "user:tool_result:workspace",
            ));
            events
        }
        _ => Vec::new(),
    }
}

/// Fold a single block of an assistant message (text, thinking, or tool_use).
fn claude_assistant_block(
    block: &Value,
    line: i64,
    ts: i64,
    pull_request_calls: &mut HashMap<String, PullRequestCommand>,
) -> Vec<HarnessSemanticEvent> {
    let object = match block.as_object() {
        Some(object) => object,
        None => return Vec::new(),
    };
    match object.get("type").and_then(Value::as_str) {
        Some("text") => {
            let text = object.get("text").and_then(Value::as_str).unwrap_or("");
            if text.is_empty() {
                Vec::new()
            } else {
                vec![semantic(
                    line,
                    ts,
                    "assistant:text",
                    "agent_message",
                    "agent",
                    serde_json::json!({ "text": text }),
                )]
            }
        }
        Some("thinking") => {
            let text = object
                .get("thinking")
                .and_then(Value::as_str)
                .or_else(|| object.get("text").and_then(Value::as_str))
                .unwrap_or("");
            if text.is_empty() {
                Vec::new()
            } else {
                vec![semantic(
                    line,
                    ts,
                    "assistant:thinking",
                    "agent_thinking",
                    "agent",
                    serde_json::json!({ "text": text }),
                )]
            }
        }
        Some("tool_use") => {
            let tool_name = object
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let call_id = object.get("id").and_then(Value::as_str).unwrap_or("");
            let input = object.get("input").cloned().unwrap_or(Value::Null);
            if matches!(tool_name, "Bash" | "Shell") && !call_id.is_empty() {
                if let Some(command) = input
                    .get("command")
                    .and_then(Value::as_str)
                    .and_then(pull_request_command)
                {
                    pull_request_calls.insert(call_id.to_string(), command);
                }
            }
            // The tool_call always goes out; the work events are additive, so a
            // `TodoWrite` reads both as a call in the transcript and as the todo
            // list it actually is.
            let mut events = vec![semantic(
                line,
                ts,
                "assistant:tool_use",
                "tool_call",
                "agent",
                tool_call_payload(call_id, tool_name, &input),
            )];
            events.extend(work_events_for_tool(
                line,
                ts,
                "assistant:tool_use",
                call_id,
                tool_name,
                &input,
            ));
            events
        }
        _ => Vec::new(),
    }
}

/// Fold Claude's `system` init banner into a `session_info` event.
///
/// Only the init subtype describes the session; other system records (warnings,
/// compaction notices) carry no session facts and are left alone.
fn claude_system(
    record: &serde_json::Map<String, Value>,
    line: i64,
    ts: i64,
) -> Vec<HarnessSemanticEvent> {
    if record.get("subtype").and_then(Value::as_str) != Some("init") {
        return Vec::new();
    }
    let text = |key: &str| record.get(key).and_then(Value::as_str).map(str::to_string);
    let list = |key: &str| {
        record
            .get(key)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        // `tools` and `slash_commands` are plain strings;
                        // `mcp_servers` is a list of `{ name, status }`.
                        item.as_str().map(str::to_string).or_else(|| {
                            item.get("name").and_then(Value::as_str).map(str::to_string)
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    vec![semantic(
        line,
        ts,
        "system:init",
        kinds::SESSION_INFO,
        "agent",
        serde_json::json!({
            "model": text("model"),
            "cwd": text("cwd"),
            "session_id": text("session_id"),
            "permission_mode": text("permissionMode").or_else(|| text("permission_mode")),
            "tools": list("tools"),
            "slash_commands": list("slash_commands"),
            "mcp_servers": list("mcp_servers"),
        }),
    )]
}

/// Fold Claude's terminal `result` record into a `run_result` event: how the run
/// ended, how long it took, and what it cost.
fn claude_result(
    record: &serde_json::Map<String, Value>,
    line: i64,
    ts: i64,
) -> Vec<HarnessSemanticEvent> {
    let is_error = record
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || record.get("subtype").and_then(Value::as_str) == Some("error");
    let number = |key: &str| record.get(key).and_then(Value::as_i64);
    vec![semantic(
        line,
        ts,
        "result",
        kinds::RUN_RESULT,
        "agent",
        serde_json::json!({
            "ok": !is_error,
            "duration_ms": number("duration_ms"),
            "num_turns": number("num_turns"),
            "cost_usd": record.get("total_cost_usd").and_then(Value::as_f64),
            "summary": record.get("result").and_then(Value::as_str),
        }),
    )]
}

/// Flatten a Claude tool_result `content` (string or typed text blocks) to text.
fn flatten_claude_tool_result(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| {
                let object = item.as_object()?;
                if object.get("type").and_then(Value::as_str) != Some("text") {
                    return None;
                }
                object
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}
