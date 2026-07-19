//! Task-frame parsing: recover a [`TaskFrame`] from a decrypted body and an
//! [`AgentCapabilities`] object from a `capabilities_result` payload. Both are
//! tolerant of foreign or malformed input and never panic.

use super::types::{
    AgentCapabilities, HarnessProvider, TaskFrame, TaskFrameKind, TokenUsage, TINYPLACE_PROTO,
};

/// Parse a decrypted body into a [`TaskFrame`], or `None` when it is not one of
/// ours (plain chatter, another protocol, or a malformed frame). Never panics.
pub fn decode_task_frame(body: &str) -> Option<TaskFrame> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let obj = value.as_object()?;

    if obj.get("proto").and_then(|v| v.as_str()) != Some(TINYPLACE_PROTO) {
        return None;
    }
    let kind = TaskFrameKind::from_wire(obj.get("kind").and_then(|v| v.as_str())?)?;
    let task_id = obj.get("taskId").and_then(|v| v.as_str())?.to_string();
    let text = obj.get("text").and_then(|v| v.as_str())?.to_string();
    // Missing/non-string ts is tolerated: encoders always stamp it, but a peer
    // that drops it should not sink the whole frame.
    let ts = obj
        .get("ts")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let correlation_id = obj
        .get("correlationId")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let harness = obj
        .get("harness")
        .and_then(|v| v.as_str())
        .and_then(HarnessProvider::from_wire);
    let provider = obj
        .get("provider")
        .and_then(|v| v.as_str())
        .and_then(HarnessProvider::from_wire);
    let model = obj
        .get("model")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string);

    let usage = obj
        .get("usage")
        .and_then(|v| serde_json::from_value::<TokenUsage>(v.clone()).ok());

    Some(TaskFrame {
        proto: TINYPLACE_PROTO.to_string(),
        kind,
        task_id,
        text,
        ts,
        correlation_id,
        harness,
        provider,
        model,
        usage,
    })
}

/// Parse an [`AgentCapabilities`] object from the frame `text` payload. Tolerant
/// of unknown providers and non-string array entries (both are dropped) and
/// never panics; returns `None` only when `text` is not a JSON object.
pub fn parse_agent_capabilities(text: &str) -> Option<AgentCapabilities> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    if !value.is_object() {
        return None;
    }
    serde_json::from_value(value).ok()
}
