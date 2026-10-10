//! Fold scoped embed progress into Medulla transcript events.

use serde_json::{json, Value};

use openhuman_embed::agent_progress::AgentProgress;

use super::types::ProgressFold;
use crate::protocol::{HarnessEventKind, StatusPayload, ToolCallPayload, ToolResultPayload};

/// Tool family stamped on a local tool call.
///
/// The harness's tools are named freely and the crate publishes no family
/// taxonomy, so classifying one here would be guesswork a consumer would then
/// trust. `other` is the honest answer.
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
    /// boundary that maps to a stream frame. `TurnCompleted` is the exception
    /// that proves the text rule — the caller re-emits the completed reply as
    /// its own closing `agent_message` after the watchdog returns, so clearing
    /// the pending text here avoids doubling the turn's final words, while the
    /// reasoning is still flushed so the answer's thinking is recorded.
    pub(super) fn fold(&mut self, progress: &AgentProgress) -> Vec<(String, Value)> {
        match progress {
            // One event carries both fragments, and either may be empty — a
            // provider that streams reasoning separately sends them on distinct
            // deltas, one that does not sends only text.
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
                        // Redact before bounding: truncating first can remove
                        // the prefix that makes a credential detectable.
                        let mut snapshot = crate::daemon::status::redact_reasoning(&self.thinking);
                        retain_tail(&mut snapshot, MAX_SNAPSHOT_CHARS);
                        self.thinking.clear();
                        events.push(("agent_thinking".into(), json!({ "text": snapshot })));
                    }
                }
                if let Some(kind) = mapped {
                    if let Some(pair) = split_kind(&kind) {
                        events.push(pair);
                    }
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
            state: "running".into(),
            detail: "turn started".into(),
            active_call_id: None,
        }),
        AgentProgress::IterationStarted { iteration, .. } => {
            HarnessEventKind::Status(StatusPayload {
                state: "running".into(),
                detail: format!("model iteration {iteration}"),
                active_call_id: None,
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
            tool_kind: TOOL_KIND.into(),
            display: display_label.clone().unwrap_or_else(|| tool_name.clone()),
            input: arguments.clone(),
        }),
        AgentProgress::ToolCallCompleted {
            call_id,
            output,
            success,
            ..
        } => HarnessEventKind::ToolResult(ToolResultPayload {
            call_id: call_id.clone(),
            ok: *success,
            exit_code: None,
            is_error: !success,
            output_bytes: output.len() as i64,
            output: output.clone(),
        }),
        AgentProgress::TurnCompleted { .. } => HarnessEventKind::Status(StatusPayload {
            state: "idle".into(),
            detail: "turn completed".into(),
            active_call_id: None,
        }),
        // Everything else (middleware spans, cache hits, retry scheduling,
        // fallback selection, sub-agent internals) carries no distinct stream
        // frame in this vocabulary.
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
