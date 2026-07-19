//! Event construction helpers shared by the three provider mappers: build a
//! [`HarnessEvent`]/[`HarnessSemanticEvent`] and the tool_call / tool_result
//! payload envelopes over the shared SDK event model.

use serde_json::Value;

use crate::tinyplace_support::HarnessEvent;

use super::shared::{bound_tool_input, byte_length, normalize_tool_kind, tool_display, truncate};
use super::types::HarnessSemanticEvent;

/// Build a bare [`HarnessEvent`] with the given kind, role, and payload.
fn event(kind: &str, role: &str, payload: Value) -> HarnessEvent {
    HarnessEvent {
        kind: kind.to_string(),
        role: role.to_string(),
        payload,
        ..Default::default()
    }
}

/// Assemble a [`HarnessSemanticEvent`] from a line/timestamp/record tag plus the
/// event kind, role, and payload.
pub(super) fn semantic(
    line: i64,
    timestamp_ms: i64,
    record_type: &str,
    kind: &str,
    role: &str,
    payload: Value,
) -> HarnessSemanticEvent {
    HarnessSemanticEvent {
        line,
        timestamp_ms,
        record_type: record_type.to_string(),
        event: event(kind, role, payload),
    }
}

/// A `user_prompt` semantic event carrying human-authored prompt text.
pub(super) fn user_prompt_event(line: i64, timestamp_ms: i64, text: &str) -> HarnessSemanticEvent {
    semantic(
        line,
        timestamp_ms,
        "user:prompt",
        "user_prompt",
        "owner",
        serde_json::json!({ "text": text, "source": "human" }),
    )
}

/// The tool_result payload: success flag, truncated output, and byte length.
pub(super) fn tool_result_payload(call_id: &str, is_error: bool, output: &str) -> Value {
    serde_json::json!({
        "call_id": call_id,
        "ok": !is_error,
        "is_error": is_error,
        "output": truncate(output),
        "output_bytes": byte_length(output),
    })
}

/// The tool_call payload: normalized kind, one-line display, and bounded input.
pub(super) fn tool_call_payload(call_id: &str, tool_name: &str, input: &Value) -> Value {
    serde_json::json!({
        "call_id": call_id,
        "tool_name": tool_name,
        "tool_kind": normalize_tool_kind(tool_name),
        "display": tool_display(tool_name, input),
        "input": bound_tool_input(input),
    })
}
