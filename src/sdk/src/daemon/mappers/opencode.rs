//! OpenCode flat-run mapper (`opencode run --format json`): fold `error`,
//! `text`, `reasoning`, and `tool` part records into error, agent_message,
//! agent_thinking, tool_call, and tool_result semantic events.

use serde_json::Value;

use super::events::{semantic, tool_call_payload, tool_result_payload};
use super::shared::{parse_json_object, safe_stringify, truncate};
use super::timestamp::parse_timestamp_ms;
use super::types::HarnessSemanticEvent;

/// Tool `state.status` values that mean the call has finished (→ tool_result).
const OPENCODE_TERMINAL_STATES: [&str; 3] = ["completed", "error", "done"];

/// Map one raw OpenCode JSONL line into zero or more semantic events.
pub(super) fn opencode_events_from_line(raw: &str, line: i64) -> Vec<HarnessSemanticEvent> {
    let record = match parse_json_object(raw) {
        Some(record) => record,
        None => return Vec::new(),
    };
    let ts = parse_timestamp_ms(record.get("timestamp"));
    let record_type = record.get("type").and_then(Value::as_str);

    if record_type == Some("error") {
        let message = describe_opencode_error(record.get("error")).unwrap_or_else(|| {
            safe_stringify(
                record
                    .get("error")
                    .unwrap_or(&Value::Object(record.clone())),
            )
        });
        return vec![semantic(
            line,
            ts,
            "opencode:error",
            "error",
            "agent",
            serde_json::json!({ "message": truncate(&message), "fatal": false }),
        )];
    }

    let part = match record.get("part").and_then(Value::as_object) {
        Some(part) => part,
        None => return Vec::new(),
    };

    if record_type == Some("text") {
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            let text = text.trim();
            if !text.is_empty() {
                return vec![semantic(
                    line,
                    ts,
                    "opencode:text",
                    "agent_message",
                    "agent",
                    serde_json::json!({ "text": text }),
                )];
            }
        }
    }

    if record_type == Some("reasoning") {
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            let text = text.trim();
            if !text.is_empty() {
                return vec![semantic(
                    line,
                    ts,
                    "opencode:reasoning",
                    "agent_thinking",
                    "agent",
                    serde_json::json!({ "text": text }),
                )];
            }
        }
    }

    if let Some(tool_name) = part.get("tool").and_then(Value::as_str) {
        if !tool_name.is_empty() {
            let call_id = part.get("callID").and_then(Value::as_str).unwrap_or("");
            let state = part.get("state").and_then(Value::as_object);
            let status = state
                .and_then(|s| s.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_lowercase();
            let output = state.and_then(|s| s.get("output"));
            let terminal = OPENCODE_TERMINAL_STATES.contains(&status.as_str())
                || matches!(output, Some(v) if !v.is_null() && v.as_str() != Some(""));
            if terminal {
                let output_text = open_code_output_text(output);
                let is_error = status == "error";
                return vec![semantic(
                    line,
                    ts,
                    "opencode:tool_result",
                    "tool_result",
                    "agent",
                    tool_result_payload(call_id, is_error, &output_text),
                )];
            }
            let input = state
                .and_then(|s| s.get("input"))
                .cloned()
                .unwrap_or(Value::Null);
            return vec![semantic(
                line,
                ts,
                "opencode:tool_call",
                "tool_call",
                "agent",
                tool_call_payload(call_id, tool_name, &input),
            )];
        }
    }

    Vec::new()
}

/// Build a human-readable message from an opencode error object, combining its
/// `name`, `data.message`/`message`, and `data.ref` suffix. `None` when no
/// message text is present.
fn describe_opencode_error(error: Option<&Value>) -> Option<String> {
    let object = error?.as_object()?;
    let data = object.get("data").and_then(Value::as_object);
    let message = data
        .and_then(|d| d.get("message"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            object
                .get("message")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
        })?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(|n| format!("{n}: "))
        .unwrap_or_default();
    let reference = data
        .and_then(|d| d.get("ref"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(|r| format!(" ({r})"))
        .unwrap_or_default();
    Some(format!("{name}{message}{reference}"))
}

/// Extract text from an opencode tool `state.output` (string, or stringified).
fn open_code_output_text(output: Option<&Value>) -> String {
    match output {
        Some(Value::String(text)) => text.clone(),
        Some(value) => safe_stringify(value),
        None => String::new(),
    }
}
