//! The TUI event vocabulary: every library `CycleEvent` plus the host-sourced
//! rows (cycle framing, conversation turns, agent/session status, effects).
//! `TuiEvent` deserializes any JSON `{kind, ...}` shape, keeping unknown kinds
//! as a passthrough so a newer backend never drops rows on an older TUI.

use serde::de::{self, Deserializer};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Usage {
    #[serde(rename = "inputTokens")]
    pub input_tokens: i64,
    #[serde(rename = "outputTokens")]
    pub output_tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub name: String,
    #[serde(default)]
    pub args: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskDigest {
    #[serde(rename = "taskId")]
    pub task_id: String,
    pub status: String,
    #[serde(default)]
    pub digest: String,
    #[serde(default, rename = "resultRef", skip_serializing_if = "Option::is_none")]
    pub result_ref: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default)]
    pub depth: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeTrace {
    pub node: String,
    #[serde(default)]
    pub ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
}

/// The full event union. Unknown kinds ride through in `Unknown`.
#[derive(Debug, Clone, PartialEq)]
pub enum TuiEvent {
    InferenceStart {
        tier: String,
        op: String,
        model: Option<String>,
    },
    InferenceEnd {
        tier: String,
        op: String,
        model: Option<String>,
        duration_ms: i64,
        usage: Option<Usage>,
        content: Option<String>,
        reasoning: Option<String>,
        tool_calls: Option<Vec<ToolCall>>,
    },
    ToolCallStart {
        index: i64,
        name: String,
    },
    ToolCallDelta {
        index: i64,
        args_delta: String,
    },
    AssistantDelta {
        delta: String,
    },
    ReasoningDelta {
        delta: String,
    },
    TaskStart {
        task_id: String,
        instruction: String,
        depth: i64,
        agent_id: Option<String>,
    },
    TaskEvent {
        task_id: String,
        event_kind: String,
        content: String,
        harness: Option<String>,
    },
    TaskAttention {
        task_id: String,
        reason: String,
        content: String,
        question_id: Option<String>,
    },
    TaskComplete {
        digest: TaskDigest,
    },
    Trace {
        entry: NodeTrace,
    },
    Error {
        source: String,
        message: String,
    },
    CycleStart {
        cycle_id: String,
    },
    CycleEnd {
        cycle_id: String,
        pass_count: i64,
        duration_ms: i64,
    },
    AgentStatus {
        agent_id: String,
        availability: String,
        detail: Option<String>,
    },
    SessionEvent {
        agent_id: String,
        session_id: String,
        event_kind: String,
        content: String,
    },
    PeerSession {
        agent_id: String,
        session_id: String,
        state: String,
        harness: Option<String>,
    },
    User {
        body: String,
    },
    Assistant {
        body: String,
    },
    Effect {
        effect: Value,
    },
    Unknown {
        kind: String,
        data: Map<String, Value>,
    },
}

impl TuiEvent {
    /// The `kind` discriminator, matching the JSON tag.
    pub fn kind(&self) -> &str {
        match self {
            TuiEvent::InferenceStart { .. } => "inference_start",
            TuiEvent::InferenceEnd { .. } => "inference_end",
            TuiEvent::ToolCallStart { .. } => "tool_call_start",
            TuiEvent::ToolCallDelta { .. } => "tool_call_delta",
            TuiEvent::AssistantDelta { .. } => "assistant_delta",
            TuiEvent::ReasoningDelta { .. } => "reasoning_delta",
            TuiEvent::TaskStart { .. } => "task_start",
            TuiEvent::TaskEvent { .. } => "task_event",
            TuiEvent::TaskAttention { .. } => "task_attention",
            TuiEvent::TaskComplete { .. } => "task_complete",
            TuiEvent::Trace { .. } => "trace",
            TuiEvent::Error { .. } => "error",
            TuiEvent::CycleStart { .. } => "cycle_start",
            TuiEvent::CycleEnd { .. } => "cycle_end",
            TuiEvent::AgentStatus { .. } => "agent_status",
            TuiEvent::SessionEvent { .. } => "session_event",
            TuiEvent::PeerSession { .. } => "peer_session",
            TuiEvent::User { .. } => "user",
            TuiEvent::Assistant { .. } => "assistant",
            TuiEvent::Effect { .. } => "effect",
            TuiEvent::Unknown { kind, .. } => kind,
        }
    }

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

fn get_str(m: &Map<String, Value>, k: &str) -> String {
    m.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}
fn opt_str(m: &Map<String, Value>, k: &str) -> Option<String> {
    m.get(k)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}
fn get_i64(m: &Map<String, Value>, k: &str) -> i64 {
    m.get(k).and_then(Value::as_i64).unwrap_or(0)
}
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

/// A sequenced event with its wall-clock timestamp (epoch ms).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventEnvelope {
    pub seq: u64,
    pub at: i64,
    pub event: TuiEvent,
}

/// The chat as plain text for the clipboard: speaker-tagged turns with their
/// original line breaks. User turns are `> `-quoted; a blank line becomes `>`.
pub fn chat_transcript(events: &[EventEnvelope]) -> String {
    let mut out: Vec<String> = Vec::new();
    for env in events {
        match &env.event {
            TuiEvent::User { body } => {
                let quoted = body
                    .split('\n')
                    .map(|line| {
                        if line.is_empty() {
                            ">".to_string()
                        } else {
                            format!("> {line}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                out.push(quoted);
            }
            TuiEvent::Assistant { body } => out.push(body.clone()),
            TuiEvent::Error { source, message } => out.push(format!("[error] {source}: {message}")),
            _ => {}
        }
    }
    out.join("\n\n")
}

/// The most recent assistant reply, for `/copy last`.
pub fn last_assistant_message(events: &[EventEnvelope]) -> Option<String> {
    events.iter().rev().find_map(|env| match &env.event {
        TuiEvent::Assistant { body } => Some(body.clone()),
        _ => None,
    })
}

/// A one-line description for the Trace / Overview lists.
pub fn describe_event(event: &TuiEvent) -> String {
    match event {
        TuiEvent::CycleStart { cycle_id } => format!("cycle started {cycle_id}"),
        TuiEvent::CycleEnd {
            pass_count,
            duration_ms,
            ..
        } => format!("cycle finished · {pass_count} passes · {duration_ms}ms"),
        TuiEvent::InferenceStart { tier, op, model } => {
            format!("{tier}/{op} → {}", model.as_deref().unwrap_or(tier))
        }
        TuiEvent::InferenceEnd {
            tier,
            op,
            duration_ms,
            ..
        } => format!("{tier}/{op} ← {duration_ms}ms"),
        TuiEvent::ToolCallStart { name, index } => format!("tool {name} (call {index})"),
        TuiEvent::ToolCallDelta { args_delta, index } => {
            format!("tool args +{}b (call {index})", args_delta.len())
        }
        TuiEvent::AssistantDelta { delta } => format!("assistant +{}b", delta.len()),
        TuiEvent::ReasoningDelta { delta } => format!("reasoning +{}b", delta.len()),
        TuiEvent::TaskStart { task_id, depth, .. } => format!("{task_id} started · depth {depth}"),
        TuiEvent::TaskEvent {
            task_id,
            event_kind,
            content,
            ..
        } => format!("{task_id} · {event_kind}: {content}"),
        TuiEvent::TaskAttention {
            task_id,
            reason,
            content,
            ..
        } => format!("{task_id} needs attention · {reason}: {content}"),
        TuiEvent::AgentStatus {
            agent_id,
            availability,
            detail,
        } => {
            let extra = detail
                .as_deref()
                .map(|d| format!(" · {d}"))
                .unwrap_or_default();
            format!("agent {agent_id} · {availability}{extra}")
        }
        TuiEvent::PeerSession {
            agent_id,
            session_id,
            state,
            harness,
        } => {
            let h = harness
                .as_deref()
                .map(|h| format!(" · {h}"))
                .unwrap_or_default();
            format!("session {session_id} on {agent_id} · {state}{h}")
        }
        TuiEvent::SessionEvent {
            session_id,
            event_kind,
            content,
            ..
        } => format!("{session_id} · {event_kind}: {content}"),
        TuiEvent::TaskComplete { digest } => format!("{} {}", digest.task_id, digest.status),
        TuiEvent::Trace { entry } => {
            let tool = entry
                .tool
                .as_deref()
                .or(entry.op.as_deref())
                .map(|t| format!("/{t}"))
                .unwrap_or_default();
            format!("{}{} · {}ms", entry.node, tool, entry.ms)
        }
        TuiEvent::Effect { effect } => {
            let k = effect
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("effect");
            format!("effect {k}")
        }
        TuiEvent::User { body } => format!("you: {body}"),
        TuiEvent::Assistant { body } => format!("assistant: {body}"),
        TuiEvent::Error { source, message } => format!("{source}: {message}"),
        TuiEvent::Unknown { kind, .. } => format!("event {kind}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(seq: u64, event: TuiEvent) -> EventEnvelope {
        EventEnvelope { seq, at: 0, event }
    }

    #[test]
    fn transcript_quotes_user_and_verbatim_assistant() {
        let events = vec![
            env(
                1,
                TuiEvent::User {
                    body: "hi\n\nthere".into(),
                },
            ),
            env(
                2,
                TuiEvent::Assistant {
                    body: "line1\nline2".into(),
                },
            ),
            env(
                3,
                TuiEvent::Error {
                    source: "cycle".into(),
                    message: "boom".into(),
                },
            ),
        ];
        let t = chat_transcript(&events);
        assert_eq!(t, "> hi\n>\n> there\n\nline1\nline2\n\n[error] cycle: boom");
    }

    #[test]
    fn last_assistant_scans_from_end() {
        let events = vec![
            env(
                1,
                TuiEvent::Assistant {
                    body: "first".into(),
                },
            ),
            env(
                2,
                TuiEvent::Assistant {
                    body: "second".into(),
                },
            ),
        ];
        assert_eq!(last_assistant_message(&events).as_deref(), Some("second"));
    }

    #[test]
    fn unknown_kind_round_trips() {
        let json = r#"{"kind":"weird_kind","payload":42}"#;
        let ev: TuiEvent = serde_json::from_str(json).unwrap();
        match &ev {
            TuiEvent::Unknown { kind, data } => {
                assert_eq!(kind, "weird_kind");
                assert_eq!(data.get("payload").unwrap(), &json!(42));
            }
            _ => panic!("expected unknown"),
        }
        let back = serde_json::to_value(&ev).unwrap();
        assert_eq!(back.get("kind").unwrap(), &json!("weird_kind"));
        assert_eq!(back.get("payload").unwrap(), &json!(42));
    }

    #[test]
    fn known_event_round_trips() {
        let ev = TuiEvent::InferenceEnd {
            tier: "reasoning".into(),
            op: "execute_step".into(),
            model: Some("gpt".into()),
            duration_ms: 120,
            usage: Some(Usage {
                input_tokens: 10,
                output_tokens: 5,
            }),
            content: None,
            reasoning: None,
            tool_calls: None,
        };
        let s = serde_json::to_string(&ev).unwrap();
        let back: TuiEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(ev, back);
    }
}
