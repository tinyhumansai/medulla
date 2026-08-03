//! Folding ACP session updates into Medulla semantic harness events.

use std::time::Instant;

use agent_client_protocol::schema::v1::SessionUpdate;
use serde_json::{json, Value};

use crate::daemon::mappers::HarnessSemanticEvent;
use crate::protocol::HarnessEvent;

use super::super::types::OnEvent;
use super::types::FoldState;

impl FoldState {
    pub(super) fn new(on_event: Option<OnEvent>) -> Self {
        Self {
            text: String::new(),
            thought: String::new(),
            events: 0,
            on_event,
            last_activity: Instant::now(),
            tool_calls: Default::default(),
        }
    }

    /// Fold a standard ACP update into Medulla's existing semantic event model.
    pub(super) fn fold(&mut self, update: SessionUpdate) {
        self.last_activity = Instant::now();
        let value = serde_json::to_value(&update).unwrap_or(Value::Null);
        let kind = value
            .get("sessionUpdate")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        // Usage telemetry is invisible to the copilot and does not end a
        // reasoning stream. Preserve the cumulative snapshot across it so the
        // next visible thought replaces the row with the complete text.
        if !matches!(kind, "agent_thought_chunk" | "usage_update") {
            self.thought.clear();
        }
        let (event_kind, role, payload) = match kind {
            "agent_message_chunk" => {
                let text = content_text(value.get("content"));
                self.text.push_str(&text);
                ("agent_message", "agent", json!({ "text": text }))
            }
            "agent_thought_chunk" => {
                self.thought.push_str(&content_text(value.get("content")));
                self.thought = crate::daemon::status::redact_reasoning(&self.thought);
                retain_tail(&mut self.thought, 780);
                ("agent_thought", "agent", json!({ "text": self.thought }))
            }
            "tool_call" => ("tool_call", "agent", self.tool_call_payload(&value)),
            "tool_call_update"
                if !matches!(
                    value.get("status").and_then(Value::as_str),
                    Some("completed" | "failed")
                ) =>
            {
                let payload = self.tool_call_payload(&value);
                if value.get("rawInput").is_none() {
                    return;
                }
                ("tool_call", "agent", payload)
            }
            "tool_call_update" => {
                let call_id = value
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.tool_calls.remove(call_id);
                (
                    "tool_result",
                    "tool",
                    json!({
                        "call_id": value.get("toolCallId"),
                        "ok": value.get("status").and_then(Value::as_str) == Some("completed"),
                        "is_error": value.get("status").and_then(Value::as_str) == Some("failed"),
                        "status": value.get("status"),
                        "output": value.get("rawOutput"),
                    }),
                )
            }
            "plan" => ("plan", "agent", value.clone()),
            "usage_update" => ("usage", "system", value.clone()),
            _ => ("status", "system", value.clone()),
        };
        let semantic = HarnessSemanticEvent {
            line: self.events as i64,
            timestamp_ms: now_ms(),
            record_type: format!("acp:{kind}"),
            event: HarnessEvent {
                kind: event_kind.to_string(),
                role: role.to_string(),
                payload,
                ..Default::default()
            },
        };
        self.events += 1;
        if let Some(callback) = self.on_event.as_mut() {
            callback(&semantic);
        }
    }

    pub(super) fn reply(&self) -> String {
        if self.text.trim().is_empty() {
            "ACP agent completed without a text response.".to_string()
        } else {
            self.text.clone()
        }
    }

    /// Merge a partial ACP tool update and expose the complete call to Medulla.
    fn tool_call_payload(&mut self, value: &Value) -> Value {
        let call_id = value
            .get("toolCallId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let call = self.tool_calls.entry(call_id.clone()).or_default();
        if let Some(title) = value.get("title").and_then(Value::as_str) {
            call.title = title.to_string();
        }
        if let Some(kind) = value.get("kind").and_then(Value::as_str) {
            call.kind = kind.to_string();
        }
        if let Some(input) = value.get("rawInput") {
            call.input = input.clone();
        }
        json!({
            "call_id": call_id,
            "tool_name": call.kind,
            "display": call.title,
            "input": call.input,
        })
    }
}

fn content_text(content: Option<&Value>) -> String {
    content
        .and_then(|value| value.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Bound a streamed snapshot while retaining the most recent reasoning.
fn retain_tail(value: &mut String, max_chars: usize) {
    if value.chars().count() <= max_chars {
        return;
    }
    let keep = max_chars.saturating_sub(1);
    let tail = value
        .chars()
        .rev()
        .take(keep)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    *value = format!("…{tail}");
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}
