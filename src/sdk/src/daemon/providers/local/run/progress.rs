//! Translating the tinyagents [`AgentEvent`] stream into Medulla's semantic
//! event vocabulary.
//!
//! A spawned CLI harness reports what it is doing by printing JSONL that
//! [`crate::daemon::mappers`] folds into [`HarnessEventKind`] values. The local
//! harness reports the same facts in-process, as typed enum variants, so this
//! module is the in-process equivalent of a line mapper: same destination
//! vocabulary, different source.
//!
//! The destination vocabulary is what makes the run view provider-agnostic — an
//! operator reading a transcript should not be able to tell whether the turn ran
//! in a child process or in this one.
//!
//! One shaping decision is deliberate. TinyAgents streams a `ModelDelta` per
//! token, and this fold feeds a *bounded* transcript — see
//! [`crate::harness_transcript`] — plus a status path that treats an
//! `agent_thinking` event as the whole reasoning so far. Emitting one event per
//! token would exhaust the transcript's cap before the turn ended and hand the
//! throttler a fragment where it expects a cumulative snapshot, so deltas are
//! accumulated here and emitted whole at the next structural boundary.

use serde_json::{json, Value};

use tinyagents::events::AgentEvent;

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
    pub(super) fn fold(&mut self, progress: &AgentEvent) -> Vec<(String, Value)> {
        match progress {
            // One event carries both fragments, and either may be empty — a
            // provider that streams reasoning separately sends them on distinct
            // deltas, one that does not sends only text.
            AgentEvent::ModelDelta { delta, .. } => {
                self.text.push_str(&delta.text);
                self.thinking.push_str(&delta.reasoning);
                Vec::new()
            }
            boundary => {
                let mapped = event_kind(boundary);
                let mut events = Vec::new();
                if mapped.is_some() {
                    if matches!(boundary, AgentEvent::RunCompleted { .. }) {
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
fn event_kind(progress: &AgentEvent) -> Option<HarnessEventKind> {
    let kind = match progress {
        AgentEvent::RunStarted { .. } => HarnessEventKind::Status(StatusPayload {
            state: "running".to_string(),
            detail: "turn started".to_string(),
            active_call_id: None,
        }),
        AgentEvent::ModelStarted { model, .. } => HarnessEventKind::Status(StatusPayload {
            state: "running".to_string(),
            detail: format!("calling {model}"),
            active_call_id: None,
        }),
        // Announced at the start so a long build shows as running rather than
        // as a gap. The arguments are not on this event — tinyagents carries
        // them on the completion — so `input` is null here and the tool result
        // below is what shows what was actually run.
        AgentEvent::ToolStarted {
            call_id, tool_name, ..
        } => HarnessEventKind::ToolCall(ToolCallPayload {
            call_id: call_id.to_string(),
            tool_name: tool_name.clone(),
            tool_kind: TOOL_KIND.to_string(),
            display: tool_name.clone(),
            input: Value::Null,
        }),
        AgentEvent::ToolCompleted {
            call_id,
            output,
            error,
            ..
        } => {
            let text = output.as_ref().map(render_output).unwrap_or_default();
            HarnessEventKind::ToolResult(ToolResultPayload {
                call_id: call_id.to_string(),
                ok: error.is_none(),
                // The tools run in-process; there is no exit status to report,
                // and inventing 0/1 from the error flag would read as one.
                exit_code: None,
                is_error: error.is_some(),
                // Byte length of what we carry, not a character count, which
                // under-reports any non-ASCII output.
                output_bytes: text.len() as i64,
                output: text,
            })
        }
        // Distinct from a completion carrying an error: that one was fed back
        // to the model, which got to react. This means the run is aborting, so
        // it is reported as a failed result rather than as ordinary output.
        AgentEvent::ToolFailed {
            call_id,
            tool_name,
            error,
            ..
        } => HarnessEventKind::ToolResult(ToolResultPayload {
            call_id: call_id.to_string(),
            ok: false,
            exit_code: None,
            is_error: true,
            output_bytes: error.len() as i64,
            output: format!("{tool_name}: {error}"),
        }),
        AgentEvent::RunCompleted { .. } => HarnessEventKind::Status(StatusPayload {
            state: "idle".to_string(),
            detail: "turn completed".to_string(),
            active_call_id: None,
        }),
        // Everything else (middleware spans, cache hits, retry scheduling,
        // fallback selection, sub-agent internals) carries no distinct stream
        // frame in this vocabulary.
        _ => return None,
    };
    Some(kind)
}

/// A tool result's captured output as text.
///
/// The capture is `serde_json::Value` because a tool may answer structurally,
/// but the overwhelmingly common case is a JSON string holding the tool's own
/// text — and rendering that through `to_string` would show an operator a
/// quoted, backslash-escaped version of output they can otherwise read.
fn render_output(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
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
