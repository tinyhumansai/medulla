//! Task-frame parsing: recover a [`TaskFrame`] from a decrypted body and an
//! [`AgentCapabilities`] object from a `capabilities_result` payload. Both are
//! tolerant of foreign or malformed input and never panic.

use super::types::{
    AgentCapabilities, HarnessProvider, HarnessTransport, TaskFrame, TaskFrameKind, TokenUsage,
    MEDULLA_TASK_PROTO,
};

/// Parse a decrypted body into a [`TaskFrame`], or `None` when it is not one of
/// ours (plain chatter, another protocol, or a malformed frame). Never panics.
pub fn decode_task_frame(body: &str) -> Option<TaskFrame> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let obj = value.as_object()?;

    if obj.get("proto").and_then(|v| v.as_str()) != Some(MEDULLA_TASK_PROTO) {
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
    let workflow_node = obj
        .get("workflowNode")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    let usage = obj
        .get("usage")
        .and_then(|v| serde_json::from_value::<TokenUsage>(v.clone()).ok());

    // A malformed work snapshot is dropped rather than sinking the frame: the
    // task result is the payload that matters, and a peer on a newer shape must
    // not make its replies unreadable here.
    let work = obj
        .get("work")
        .and_then(|v| serde_json::from_value::<crate::harness_work::WorkSnapshot>(v.clone()).ok())
        .map(Box::new);

    // A blank workflow id is treated as absent: an encoder that always writes
    // the key should not turn every task into a lookup for "".
    let workflow = obj
        .get("workflow")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string);
    let workflow_fingerprint = obj
        .get("workflowFingerprint")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|fingerprint| !fingerprint.is_empty())
        .map(str::to_string);
    // An unrecognized transport fails closed. A worker that ran the CLI because
    // it did not understand the flavor would be a silent downgrade — the
    // operator asked for a shared process and would get a fork per task with no
    // way to tell. Absent, however, is simply the provider default.
    let transport = match obj.get("transport") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(value)) => Some(HarnessTransport::from_wire(value.trim())?),
        Some(_) => return None,
    };
    let custom_harness = obj
        .get("customHarness")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(Box::<str>::from);

    // Blank is absent for compatibility with encoders that always write the
    // key. Unknown non-empty modes fail closed: silently turning one into the
    // full tool surface could grant a review turn authoring capabilities.
    let tool_mode = match obj.get("tool_mode") {
        None => None,
        Some(serde_json::Value::String(mode)) if mode.trim().is_empty() => None,
        Some(serde_json::Value::String(mode)) => {
            let mode = mode.trim();
            let valid_scoped_propose = mode
                .strip_prefix("propose:")
                .is_some_and(|scope| !scope.trim().is_empty());
            if matches!(mode, "full" | "propose") || valid_scoped_propose {
                Some(mode.to_string())
            } else {
                return None;
            }
        }
        Some(_) => return None,
    };

    // Blank is absent, for the same reason a blank workflow id is: an encoder
    // that always writes the key must not put every task into a conversation
    // named "" — which would be one shared context for every unrelated sender.
    let conversation = obj
        .get("conversation")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string);
    // Missing means operator-started work for compatibility with older peers.
    // A present value must fit exactly; narrowing with `as` would turn 256 into
    // root depth zero and let a malformed frame widen delegated capabilities.
    let fleet_depth = match obj.get("fleet_depth") {
        None => 0,
        Some(value) => u8::try_from(value.as_u64()?).ok()?,
    };
    let workflow_inputs = match obj.get("inputs") {
        None => serde_json::Map::new(),
        Some(serde_json::Value::Object(inputs)) => inputs.clone(),
        Some(_) => return None,
    };
    // Decoded so a *response* can report which session served the task. Nothing
    // on the receiving side of a `task` frame reads it: honouring an inbound
    // session id is session targeting, which is deliberately not implemented —
    // one caller must not be able to run inside a session opened for another.
    // Blank is treated as absent, so a sender that always writes the key does
    // not report a session nothing can resume.
    let session_id = obj
        .get("sessionId")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string);

    Some(TaskFrame {
        proto: MEDULLA_TASK_PROTO.to_string(),
        kind,
        task_id,
        text,
        ts,
        correlation_id,
        harness,
        provider,
        transport,
        custom_harness,
        model,
        tool_mode,
        workflow_node,
        workflow,
        workflow_fingerprint,
        workflow_inputs,
        conversation,
        fleet_depth,
        usage,
        work,
        session_id,
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
