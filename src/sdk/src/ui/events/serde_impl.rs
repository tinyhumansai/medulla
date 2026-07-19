//! Custom `Serialize`/`Deserialize` for [`TuiEvent`].
//!
//! `TuiEvent` serializes to a compact `{kind, ...}` JSON object (camelCase keys,
//! null fields dropped) and deserializes any such object — keeping unrecognized
//! kinds as [`TuiEvent::Unknown`] so a newer backend never drops rows on an older
//! TUI. The field-extraction helpers below tolerate missing or ill-typed fields.

use serde::de::{self, Deserializer};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::types::TuiEvent;

impl TuiEvent {
    /// Render the event to its compact JSON object, including the `kind` tag and
    /// with null fields dropped to keep the shape TS-friendly.
    fn to_value(&self) -> Value {
        let mut v = match self {
            TuiEvent::InferenceStart { tier, op, model } => {
                json!({ "tier": tier, "op": op, "model": model })
            }
            TuiEvent::InferenceEnd {
                tier,
                op,
                model,
                duration_ms,
                usage,
                content,
                reasoning,
                tool_calls,
            } => json!({
                "tier": tier, "op": op, "model": model, "durationMs": duration_ms,
                "usage": usage, "content": content, "reasoning": reasoning,
                "toolCalls": tool_calls,
            }),
            TuiEvent::ToolCallStart { index, name } => json!({ "index": index, "name": name }),
            TuiEvent::ToolCallDelta { index, args_delta } => {
                json!({ "index": index, "argsDelta": args_delta })
            }
            TuiEvent::AssistantDelta { delta } => json!({ "delta": delta }),
            TuiEvent::ReasoningDelta { delta } => json!({ "delta": delta }),
            TuiEvent::TaskStart {
                task_id,
                instruction,
                depth,
                agent_id,
            } => {
                json!({ "taskId": task_id, "instruction": instruction, "depth": depth, "agentId": agent_id })
            }
            TuiEvent::TaskEvent {
                task_id,
                event_kind,
                content,
                harness,
            } => {
                json!({ "taskId": task_id, "eventKind": event_kind, "content": content, "harness": harness })
            }
            TuiEvent::TaskAttention {
                task_id,
                reason,
                content,
                question_id,
            } => {
                json!({ "taskId": task_id, "reason": reason, "content": content, "questionId": question_id })
            }
            TuiEvent::TaskComplete { digest } => json!({ "digest": digest }),
            TuiEvent::Trace { entry } => json!({ "entry": entry }),
            TuiEvent::Error { source, message } => json!({ "source": source, "message": message }),
            TuiEvent::CycleStart { cycle_id } => json!({ "cycleId": cycle_id }),
            TuiEvent::CycleEnd {
                cycle_id,
                pass_count,
                duration_ms,
            } => json!({ "cycleId": cycle_id, "passCount": pass_count, "durationMs": duration_ms }),
            TuiEvent::AgentStatus {
                agent_id,
                availability,
                detail,
            } => json!({ "agentId": agent_id, "availability": availability, "detail": detail }),
            TuiEvent::SessionEvent {
                agent_id,
                session_id,
                event_kind,
                content,
            } => {
                json!({ "agentId": agent_id, "sessionId": session_id, "eventKind": event_kind, "content": content })
            }
            TuiEvent::PeerSession {
                agent_id,
                session_id,
                state,
                harness,
            } => {
                json!({ "agentId": agent_id, "sessionId": session_id, "state": state, "harness": harness })
            }
            TuiEvent::User { body } => json!({ "body": body }),
            TuiEvent::Assistant { body } => json!({ "body": body }),
            TuiEvent::Effect { effect } => json!({ "effect": effect }),
            TuiEvent::Unknown { data, .. } => Value::Object(data.clone()),
        };
        if let Value::Object(map) = &mut v {
            map.insert("kind".into(), Value::String(self.kind().to_string()));
            // Drop nulls to keep the JSON compact and TS-shaped.
            map.retain(|_, val| !val.is_null());
        }
        v
    }
}

impl Serialize for TuiEvent {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.to_value().serialize(s)
    }
}

/// Read a required string field, defaulting to `""` when missing or non-string.
fn get_str(m: &Map<String, Value>, k: &str) -> String {
    m.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}
/// Read an optional string field: missing, non-string, or empty all map to `None`.
fn opt_str(m: &Map<String, Value>, k: &str) -> Option<String> {
    m.get(k)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}
/// Read a required integer field, defaulting to `0` when missing or non-integer.
fn get_i64(m: &Map<String, Value>, k: &str) -> i64 {
    m.get(k).and_then(Value::as_i64).unwrap_or(0)
}
/// Deserialize a nested field into `T`, yielding `None` on any decode failure.
fn from_field<T: for<'d> Deserialize<'d>>(m: &Map<String, Value>, k: &str) -> Option<T> {
    m.get(k)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

impl<'de> Deserialize<'de> for TuiEvent {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(d)?;
        let map = value
            .as_object()
            .ok_or_else(|| de::Error::custom("event must be an object"))?;
        let kind = map.get("kind").and_then(Value::as_str).unwrap_or("");
        Ok(match kind {
            "inference_start" => TuiEvent::InferenceStart {
                tier: get_str(map, "tier"),
                op: get_str(map, "op"),
                model: opt_str(map, "model"),
            },
            "inference_end" => TuiEvent::InferenceEnd {
                tier: get_str(map, "tier"),
                op: get_str(map, "op"),
                model: opt_str(map, "model"),
                duration_ms: get_i64(map, "durationMs"),
                usage: from_field(map, "usage"),
                content: opt_str(map, "content"),
                reasoning: opt_str(map, "reasoning"),
                tool_calls: from_field(map, "toolCalls"),
            },
            "tool_call_start" => TuiEvent::ToolCallStart {
                index: get_i64(map, "index"),
                name: get_str(map, "name"),
            },
            "tool_call_delta" => TuiEvent::ToolCallDelta {
                index: get_i64(map, "index"),
                args_delta: get_str(map, "argsDelta"),
            },
            "assistant_delta" => TuiEvent::AssistantDelta {
                delta: get_str(map, "delta"),
            },
            "reasoning_delta" => TuiEvent::ReasoningDelta {
                delta: get_str(map, "delta"),
            },
            "task_start" => TuiEvent::TaskStart {
                task_id: get_str(map, "taskId"),
                instruction: get_str(map, "instruction"),
                depth: get_i64(map, "depth"),
                agent_id: opt_str(map, "agentId"),
            },
            "task_event" => TuiEvent::TaskEvent {
                task_id: get_str(map, "taskId"),
                event_kind: get_str(map, "eventKind"),
                content: get_str(map, "content"),
                harness: opt_str(map, "harness"),
            },
            "task_attention" => TuiEvent::TaskAttention {
                task_id: get_str(map, "taskId"),
                reason: get_str(map, "reason"),
                content: get_str(map, "content"),
                question_id: opt_str(map, "questionId"),
            },
            "task_complete" => TuiEvent::TaskComplete {
                digest: from_field(map, "digest")
                    .ok_or_else(|| de::Error::custom("task_complete needs digest"))?,
            },
            "trace" => TuiEvent::Trace {
                entry: from_field(map, "entry")
                    .ok_or_else(|| de::Error::custom("trace needs entry"))?,
            },
            "error" => TuiEvent::Error {
                source: get_str(map, "source"),
                message: get_str(map, "message"),
            },
            "cycle_start" => TuiEvent::CycleStart {
                cycle_id: get_str(map, "cycleId"),
            },
            "cycle_end" => TuiEvent::CycleEnd {
                cycle_id: get_str(map, "cycleId"),
                pass_count: get_i64(map, "passCount"),
                duration_ms: get_i64(map, "durationMs"),
            },
            "agent_status" => TuiEvent::AgentStatus {
                agent_id: get_str(map, "agentId"),
                availability: get_str(map, "availability"),
                detail: opt_str(map, "detail"),
            },
            "session_event" => TuiEvent::SessionEvent {
                agent_id: get_str(map, "agentId"),
                session_id: get_str(map, "sessionId"),
                event_kind: get_str(map, "eventKind"),
                content: get_str(map, "content"),
            },
            "peer_session" => TuiEvent::PeerSession {
                agent_id: get_str(map, "agentId"),
                session_id: get_str(map, "sessionId"),
                state: get_str(map, "state"),
                harness: opt_str(map, "harness"),
            },
            "user" => TuiEvent::User {
                body: get_str(map, "body"),
            },
            "assistant" => TuiEvent::Assistant {
                body: get_str(map, "body"),
            },
            "effect" => TuiEvent::Effect {
                effect: map.get("effect").cloned().unwrap_or(Value::Null),
            },
            other => TuiEvent::Unknown {
                kind: other.to_string(),
                data: map.clone(),
            },
        })
    }
}
