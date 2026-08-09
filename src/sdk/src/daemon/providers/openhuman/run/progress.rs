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
//!
//! One difference is deliberate. The bridge forwards each `TextDelta` /
//! `ThinkingDelta` as its own event because the socket has no downstream bound
//! to protect, but this fold also feeds a *bounded* transcript — see
//! [`crate::harness_transcript`] — and a status path that treats an
//! `agent_thinking` event as the whole reasoning so far. So the deltas are
//! accumulated here and emitted whole at the next phase boundary, in the shape
//! that mirror targets.

use serde_json::Value;

use super::core_contract::AgentProgress;
use super::types::ProgressFold;
use crate::protocol::{
    ApprovalRequestPayload, HarnessEventKind, StatusPayload, ToolCallPayload, ToolResultPayload,
};

/// Tool family stamped on an OpenHuman tool call.
///
/// The core's tools are named freely (`bash`, `read_file`, an MCP tool's own
/// name), and it publishes no family taxonomy, so classifying one here would be
/// guesswork that a consumer would then trust. `other` is the honest answer and
/// the same one the socket bridge gives.
const TOOL_KIND: &str = "other";

/// Characters of reasoning kept in one emitted thinking snapshot.
///
/// The same bound the ACP fold applies (`retain_tail(780)`): most of a
/// reasoning block is the model working outward from its premise, and once the
/// most recent 780 characters still show what it concluded, that is enough for
/// an operator deciding whether to let the turn continue.
const MAX_SNAPSHOT_CHARS: usize = 780;

impl ProgressFold {
    /// Fold one progress event into the semantic events it completes.
    ///
    /// `TextDelta` / `ThinkingDelta` accumulate into the fold and complete
    /// nothing by themselves; the next *structural* event — a tool call, an
    /// iteration, an approval gate, the turn ending — closes the utterance and
    /// emits it whole: one `agent_message` per completed message, and one
    /// `agent_thinking` carrying the full reasoning snapshot, redacted and
    /// bounded, so the status throttler's newest-is-the-whole assumption holds.
    ///
    /// Telemetry sitting between tokens (`TurnCostUpdated`) is not a boundary:
    /// it must not split a message in half, so nothing is flushed until a
    /// boundary that mapps to a stream frame. `TurnCompleted` is the exception
    /// that proves the text rule — the caller re-emits the completed reply as
    /// its own closing `agent_message` after the watchdog returns, so clearing
    /// the pending text here avoids doubling the turn's final words, while the
    /// reasoning is still flushed so the answer's thinking is recorded.
    pub(super) fn fold(&mut self, progress: &AgentProgress) -> Vec<(String, Value)> {
        match progress {
            AgentProgress::TextDelta { delta, .. } => {
                self.text.push_str(delta);
                Vec::new()
            }
            AgentProgress::ThinkingDelta { delta, .. } => {
                self.thinking.push_str(delta);
                Vec::new()
            }
            boundary => {
                let mapped = event_kind(boundary);
                let mut events = Vec::new();
                if mapped.is_some() {
                    if matches!(boundary, AgentProgress::TurnCompleted { .. }) {
                        self.text.clear();
                    } else if !self.text.is_empty() {
                        events.push((
                            "agent_message".into(),
                            json!({ "text": std::mem::take(&mut self.text) }),
                        ));
                    }
                    if !self.thinking.is_empty() {
                        let snapshot = crate::daemon::status::redact_reasoning(&self.thinking);
                        self.thinking.clear();
                        let mut snapshot = snapshot;
                        retain_tail(&mut snapshot, MAX_SNAPSHOT_CHARS);
                        events.push(("agent_thinking".into(), json!({ "text": snapshot })));
                    }
                }
                if let Some(kind) = mapped {
                    events.extend(split_kind(&kind).into_iter().collect::<Vec<_>>());
                }
                events
            }
        }
    }
}

/// The typed event one progress variant folds into, when it folds into one.
///
/// Delta variants are handled by [`ProgressFold::fold`] before they reach this
/// matcher, so none are listed here.
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
            ..
        } => HarnessEventKind::ToolResult(ToolResultPayload {
            call_id: call_id.clone(),
            ok: *success,
            // The core runs its tools in-process; there is no exit status to
            // report, and inventing 0/1 from `success` would read as one.
            exit_code: None,
            is_error: !*success,
            // `output_bytes` is the byte length of the output, and status
            // consumers render it as such — derive it from the bytes we carry
            // rather than the core's character count, which under-reports any
            // non-ASCII output.
            output: output.clone(),
            output_bytes: output.len() as i64,
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
        // Everything else (arg deltas, cost/usage rollups, per-call model
        // accounting, subagent-internal frames, task-board writes, raw
        // TurnContent) carries no distinct stream frame in this vocabulary.
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

/// Bound a reasoning snapshot while retaining the most recent text.
///
/// The most recent characters are the conclusion, which is the part worth
/// keeping; the slab of working-out before them is what gets dropped. Applied
/// after redaction — truncating first can remove the prefix that makes a
/// credential detectable.
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