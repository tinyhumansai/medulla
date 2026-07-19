//! Pure, read-only derivations over events: the [`TuiEvent::kind`] discriminator
//! plus the transcript, last-message, and one-line-description helpers the UI
//! layers build on. Nothing here mutates state or performs I/O.

use serde_json::Value;

use super::types::{EventEnvelope, TuiEvent};

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
