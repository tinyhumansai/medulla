//! Claude flat-run mapper (`claude -p --output-format stream-json`): fold a user
//! or assistant transcript record into user_prompt, agent_message,
//! agent_thinking, tool_call, and tool_result semantic events.

use serde_json::Value;

use super::events::{semantic, tool_call_payload, tool_result_payload, user_prompt_event};
use super::shared::{as_array, parse_json_object};
use super::timestamp::parse_timestamp_ms;
use super::types::HarnessSemanticEvent;

/// Map one raw Claude JSONL line into zero or more semantic events.
pub(super) fn claude_events_from_line(raw: &str, line: i64) -> Vec<HarnessSemanticEvent> {
    let record = match parse_json_object(raw) {
        Some(record) => record,
        None => return Vec::new(),
    };
    let ts = parse_timestamp_ms(record.get("timestamp"));
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
            .flat_map(|block| claude_user_block(block, line, ts))
            .collect();
    }

    if record_type == Some("assistant") && source_role == Some("assistant") {
        return as_array(message.get("content"))
            .iter()
            .flat_map(|block| claude_assistant_block(block, line, ts))
            .collect();
    }

    Vec::new()
}

/// Fold a single block of a user message (text prompt or tool_result).
fn claude_user_block(block: &Value, line: i64, ts: i64) -> Vec<HarnessSemanticEvent> {
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
            vec![semantic(
                line,
                ts,
                "user:tool_result",
                "tool_result",
                "agent",
                tool_result_payload(call_id, is_error, &output),
            )]
        }
        _ => Vec::new(),
    }
}

/// Fold a single block of an assistant message (text, thinking, or tool_use).
fn claude_assistant_block(block: &Value, line: i64, ts: i64) -> Vec<HarnessSemanticEvent> {
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
            vec![semantic(
                line,
                ts,
                "assistant:tool_use",
                "tool_call",
                "agent",
                tool_call_payload(call_id, tool_name, &input),
            )]
        }
        _ => Vec::new(),
    }
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
