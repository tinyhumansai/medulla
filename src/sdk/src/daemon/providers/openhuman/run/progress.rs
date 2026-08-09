//! Translating the core's [`AgentProgress`] stream into Medulla's semantic
//! event vocabulary.
//!
//! A spawned CLI harness reports what it is doing by printing JSONL that
//! [`crate::daemon::mappers`] folds into [`HarnessEventKind`] values. The
//! embedded core reports the same facts in-process, as typed enum variants, so
//! this module is the embedded equivalent of a line mapper: same destination
//! vocabulary, different source.
//!
//! The mapping deliberately mirrors the one OpenHuman already ships for its own
//! Medulla socket bridge (`platform/socket/medulla/envelope.rs`) — an operator
//! consuming a transcript should not be able to tell whether the turn reached
//! them over a socket or through this function.

use serde_json::Value;

use crate::protocol::{
    ApprovalRequestPayload, HarnessEventKind, StatusPayload, TextPayload, ToolCallPayload,
    ToolResultPayload,
};

use super::core_contract::AgentProgress;

/// Tool family stamped on an OpenHuman tool call.
///
/// The core's tools are named freely (`bash`, `read_file`, an MCP tool's own
/// name), and it publishes no family taxonomy, so classifying one here would be
/// guesswork that a consumer would then trust. `other` is the honest answer and
/// the same one the socket bridge gives.
const TOOL_KIND: &str = "other";

/// The semantic events one progress event folds into.
///
/// Returns an empty vector for the variants that carry no user-facing stream
/// frame — argument-delta fragments, cost rollups, per-call model accounting,
/// task-board writes, sub-agent internals — so the transcript stays inside the
/// `agent_message / agent_thinking / tool_call / tool_result / status /
/// approval_request` vocabulary [`HarnessEventKind`] enumerates.
///
/// A vector rather than an [`Option`] because the fold is one-to-*many* in
/// principle: a future variant carrying both a status change and a message has
/// somewhere to put both without changing every caller.
pub(super) fn semantic_events(progress: &AgentProgress) -> Vec<(String, Value)> {
    match event_kind(progress) {
        Some(kind) => split_kind(&kind).into_iter().collect(),
        None => Vec::new(),
    }
}

/// The typed event one progress variant folds into, when it folds into one.
fn event_kind(progress: &AgentProgress) -> Option<HarnessEventKind> {
    let kind = match progress {
        AgentProgress::TurnStarted => HarnessEventKind::Status(StatusPayload {
            state: "running".to_string(),
            detail: "turn started".to_string(),
            active_call_id: None,
        }),
        AgentProgress::IterationStarted {
            iteration,
            max_iterations,
        } => HarnessEventKind::Status(StatusPayload {
            state: "running".to_string(),
            detail: format!("iteration {iteration}/{max_iterations}"),
            active_call_id: None,
        }),
        AgentProgress::TextDelta { delta, .. } => HarnessEventKind::AgentMessage(TextPayload {
            text: delta.clone(),
        }),
        AgentProgress::ThinkingDelta { delta, .. } => {
            HarnessEventKind::AgentThinking(TextPayload {
                text: delta.clone(),
            })
        }
        AgentProgress::ToolCallStarted {
            call_id,
            tool_name,
            arguments,
            display_label,
            ..
        } => HarnessEventKind::ToolCall(ToolCallPayload {
            call_id: call_id.clone(),
            tool_name: tool_name.clone(),
            tool_kind: TOOL_KIND.to_string(),
            display: display_label.clone().unwrap_or_else(|| tool_name.clone()),
            input: arguments.clone(),
        }),
        AgentProgress::ToolCallCompleted {
            call_id,
            success,
            output,
            output_chars,
            ..
        } => HarnessEventKind::ToolResult(ToolResultPayload {
            call_id: call_id.clone(),
            ok: *success,
            // The core runs its tools in-process; there is no exit status to
            // report, and inventing 0/1 from `success` would read as one.
            exit_code: None,
            is_error: !*success,
            output: output.clone(),
            output_bytes: *output_chars as i64,
        }),
        AgentProgress::SubagentAwaitingUser {
            task_id, question, ..
        } => HarnessEventKind::ApprovalRequest(ApprovalRequestPayload {
            call_id: Some(task_id.clone()),
            tool_name: "subagent".to_string(),
            display: question.clone(),
            reason: None,
        }),
        AgentProgress::TurnCompleted { .. } => HarnessEventKind::Status(StatusPayload {
            state: "idle".to_string(),
            detail: "turn completed".to_string(),
            active_call_id: None,
        }),
        _ => return None,
    };
    Some(kind)
}

/// The wire `kind` string and `payload` object of a typed event.
///
/// [`HarnessEventKind`] is adjacently tagged, so serializing it already yields
/// exactly the pair [`crate::protocol::HarnessEvent`] stores — extracted rather
/// than hand-written, so the discriminator can never drift from the enum.
fn split_kind(kind: &HarnessEventKind) -> Option<(String, Value)> {
    let tagged = serde_json::to_value(kind).ok()?;
    let name = tagged.get("kind")?.as_str()?.to_string();
    let payload = tagged.get("payload").cloned().unwrap_or(Value::Null);
    Some((name, payload))
}
