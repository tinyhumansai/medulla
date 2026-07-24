//! **The driver seam.** Sessions are driven from exactly two sources — a
//! `medulla-tinyplace/1` task frame, or a `tinyplace.harness.session.v*`
//! envelope — and this module is the one place that knows the difference.
//!
//! Everything downstream ([`SessionRegistry`](super::registry::SessionRegistry),
//! [`SessionManager`](super::manager::SessionManager), the transports) sees only
//! the normalized [`TurnRequest`]. Adding a third driver means adding a variant
//! here and a fold below; it must not mean touching the registry or the
//! transports.
//!
//! The asymmetry between the two drivers is real and deliberate:
//!
//! | | task frames | session envelopes |
//! |---|---|---|
//! | who runs the harness | this daemon | a remote wrapper |
//! | a turn opens on | `task` frame | `user_prompt` event |
//! | mid-turn steering | `input` frame | none — the transcript is a record |
//! | terminal signal | we emit `reply`/`error` | `lifecycle.turn_end` |
//! | conversation anchor | the authenticated sender | `scope.wrapper_session_id` |
//!
//! An envelope-driven session is therefore **observed, not executed**: folding
//! one never spawns a process. That is why [`fold_envelope`] returns an
//! [`EnvelopeFold`] rather than a bare [`TurnRequest`] — most envelope events
//! are progress reports on a turn somebody else is running.

use ::tinyplace::types::{
    AnySessionEnvelope, HarnessEventKind, SessionEnvelopeV1, SessionEnvelopeV2,
};

use crate::tinyplace::{HarnessProvider, TaskFrame, TaskFrameKind};

use super::routing::{route_session_class, Stimulus};
use super::types::{SessionClass, SessionKey, SessionPolicy, TurnOrigin, TurnRequest};

/// One inbound stimulus, before normalization. The two variants are the two
/// drivers named in [`SessionDriver`](super::types::SessionDriver).
#[derive(Debug, Clone)]
pub enum SessionInput {
    /// A decoded `medulla-tinyplace/1` frame from an authenticated sender.
    Frame {
        /// The authenticated sender's cryptoId. **Never** taken from the frame
        /// body — a frame cannot name its own author.
        from: String,
        /// The decoded frame.
        frame: TaskFrame,
    },
    /// A plain-text DM that carried no frame at all.
    PlainText {
        /// The authenticated sender's cryptoId.
        from: String,
        /// The message body.
        text: String,
    },
    /// A harness session envelope streamed from a wrapper.
    Envelope(Box<AnySessionEnvelope>),
}

/// What folding a [`SessionInput`] produced.
#[derive(Debug, Clone, PartialEq)]
pub enum Folded {
    /// Open a turn. The caller runs it and replies.
    Turn(Box<TurnRequest>),
    /// Steer the turn already in flight on this conversation.
    Steer {
        /// Which conversation to steer.
        key: SessionKey,
        /// The frame's task id, so the caller can match the running record.
        task_id: String,
        /// The dispatch key, when the sender set one.
        correlation_id: Option<String>,
        /// The text to feed the child's stdin.
        text: String,
    },
    /// A progress observation on a session somebody else is running. Update the
    /// record; do not spawn anything.
    Observe(Box<Observation>),
    /// Nothing actionable — a response frame we sent, or an event kind that
    /// carries no state.
    Ignore,
}

/// A state update folded out of an envelope-driven session.
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    /// The conversation this envelope belongs to.
    pub key: SessionKey,
    /// The harness's own session id, from `scope.harness_session_id`.
    pub harness_session_id: String,
    /// The working directory the observed session runs in.
    pub cwd: String,
    /// A short human-readable line describing what happened.
    pub detail: String,
    /// Whether this event ends a turn (`lifecycle.turn_end`), so the record's
    /// turn counter advances.
    pub ends_turn: bool,
    /// Whether this event reports a failure.
    pub is_error: bool,
    /// The envelope event's monotonic sequence, for ordering/dedup.
    pub seq: i64,
}

/// Resolve an envelope's wire provider name into the daemon's provider enum.
///
/// The wire field is a free string for forward-compatibility, so an unknown
/// provider folds to `fallback` rather than dropping the envelope — a newer
/// wrapper's session is still worth showing the operator.
fn provider_from_wire(provider: &str, fallback: HarnessProvider) -> HarnessProvider {
    HarnessProvider::from_wire(provider).unwrap_or(fallback)
}

/// Fold one inbound stimulus into an action.
///
/// `default_provider` serves a stimulus that names none. `policy` is the
/// operator's routing pin.
pub fn fold(
    input: SessionInput,
    default_provider: HarnessProvider,
    policy: SessionPolicy,
) -> Folded {
    match input {
        SessionInput::Frame { from, frame } => fold_frame(&from, frame, default_provider, policy),
        SessionInput::PlainText { from, text } => {
            if text.trim().is_empty() {
                return Folded::Ignore;
            }
            let key = SessionKey::new(from, default_provider);
            // A conversational DM is a conversation: it routes unbound under
            // `auto`, so the peer's next message remembers this one.
            let class = route_session_class(Stimulus::PlainText, None, policy);
            Folded::Turn(Box::new(TurnRequest {
                key,
                class,
                text,
                origin: TurnOrigin::Operator,
                model: None,
            }))
        }
        SessionInput::Envelope(envelope) => fold_envelope(&envelope),
    }
}

/// Fold a `medulla-tinyplace/1` frame.
fn fold_frame(
    from: &str,
    frame: TaskFrame,
    default_provider: HarnessProvider,
    policy: SessionPolicy,
) -> Folded {
    let provider = frame.provider.unwrap_or(default_provider);
    let key = SessionKey::new(from, provider);
    match frame.kind {
        TaskFrameKind::Task => {
            // A task frame is discrete delegated work: bounded under `auto`, so
            // two tasks never see each other's context.
            let class = route_session_class(Stimulus::Task, None, policy);
            Folded::Turn(Box::new(TurnRequest {
                key,
                class,
                text: frame.text,
                origin: TurnOrigin::Frame {
                    task_id: frame.task_id,
                    correlation_id: frame.correlation_id,
                },
                model: frame.model,
            }))
        }
        TaskFrameKind::Input => Folded::Steer {
            key,
            task_id: frame.task_id,
            correlation_id: frame.correlation_id,
            text: frame.text,
        },
        // status/reply/error/ack/capabilities* are responses or probes — not
        // turns. The capability probe has its own path in the daemon.
        _ => Folded::Ignore,
    }
}

/// Fold a session envelope.
///
/// The conversation anchor is `scope.wrapper_session_id`, falling back to
/// `scope.harness_session_id` — a wrapper may run several harness sessions under
/// one wrapper session, and the wrapper id is the stable one.
pub fn fold_envelope(envelope: &AnySessionEnvelope) -> Folded {
    match envelope {
        AnySessionEnvelope::V2(v2) => fold_envelope_v2(v2),
        AnySessionEnvelope::V1(v1) => fold_envelope_v1(v1),
    }
}

/// The conversation anchor for an envelope scope.
fn anchor(wrapper_session_id: &str, harness_session_id: &str) -> Option<String> {
    let anchor = if wrapper_session_id.is_empty() {
        harness_session_id
    } else {
        wrapper_session_id
    };
    (!anchor.is_empty()).then(|| anchor.to_string())
}

/// Fold a v2 envelope, whose typed `event` carries the semantics.
fn fold_envelope_v2(envelope: &SessionEnvelopeV2) -> Folded {
    let Some(anchor) = anchor(
        &envelope.scope.wrapper_session_id,
        &envelope.scope.harness_session_id,
    ) else {
        return Folded::Ignore;
    };
    let key = SessionKey::new(
        anchor,
        provider_from_wire(&envelope.harness.provider, HarnessProvider::Claude),
    );
    let observe = |detail: String, ends_turn: bool, is_error: bool| {
        Folded::Observe(Box::new(Observation {
            key: key.clone(),
            harness_session_id: envelope.scope.harness_session_id.clone(),
            cwd: envelope.scope.cwd.clone(),
            detail,
            ends_turn,
            is_error,
            seq: envelope.event.seq,
        }))
    };
    match envelope.event.decoded() {
        // A user prompt is the only envelope event that *opens* a turn. It is
        // still an observation, not an execution: the wrapper runs the harness.
        // Callers that want to mirror the turn locally build a `TurnRequest`
        // from `envelope_turn`.
        HarnessEventKind::UserPrompt(payload) => {
            observe(format!("prompt: {}", payload.text), false, false)
        }
        HarnessEventKind::AgentMessage(payload) => observe(payload.text, false, false),
        HarnessEventKind::AgentThinking(_) => observe("thinking".to_string(), false, false),
        HarnessEventKind::ToolCall(payload) => {
            observe(format!("tool {}", payload.tool_name), false, false)
        }
        HarnessEventKind::ToolResult(payload) => observe(
            format!("tool result{}", if payload.is_error { " ✕" } else { "" }),
            false,
            payload.is_error,
        ),
        HarnessEventKind::ApprovalRequest(payload) => {
            observe(format!("approval: {}", payload.tool_name), false, false)
        }
        HarnessEventKind::Status(payload) => observe(payload.detail, false, false),
        HarnessEventKind::Lifecycle(payload) => {
            let phase = payload.phase;
            let ends_turn = phase == "turn_end";
            observe(format!("lifecycle: {phase}"), ends_turn, false)
        }
        HarnessEventKind::Error(payload) => observe(payload.message, false, true),
        HarnessEventKind::Unknown(_) => Folded::Ignore,
    }
}

/// Fold a v1 envelope, whose `message` block is the only semantic content.
fn fold_envelope_v1(envelope: &SessionEnvelopeV1) -> Folded {
    let Some(anchor) = anchor(
        &envelope.scope.wrapper_session_id,
        &envelope.scope.harness_session_id,
    ) else {
        return Folded::Ignore;
    };
    if envelope.message.text.trim().is_empty() {
        return Folded::Ignore;
    }
    let key = SessionKey::new(
        anchor,
        provider_from_wire(&envelope.harness.provider, HarnessProvider::Claude),
    );
    Folded::Observe(Box::new(Observation {
        key,
        harness_session_id: envelope.scope.harness_session_id.clone(),
        cwd: envelope.scope.cwd.clone(),
        detail: envelope.message.text.clone(),
        ends_turn: false,
        is_error: false,
        // v1 has no per-event sequence; `line` is the closest ordering signal.
        seq: envelope.message.line,
    }))
}

/// Build a [`TurnRequest`] from an envelope's `user_prompt`, for a caller that
/// wants to *run* the prompt locally rather than merely record it.
///
/// Returns `None` for any envelope that is not a v2 `user_prompt` — the only
/// envelope event that carries an executable instruction.
pub fn envelope_turn(envelope: &AnySessionEnvelope, class: SessionClass) -> Option<TurnRequest> {
    let AnySessionEnvelope::V2(v2) = envelope else {
        return None;
    };
    let HarnessEventKind::UserPrompt(payload) = v2.event.decoded() else {
        return None;
    };
    if payload.text.trim().is_empty() {
        return None;
    }
    let anchor = anchor(&v2.scope.wrapper_session_id, &v2.scope.harness_session_id)?;
    Some(TurnRequest {
        key: SessionKey::new(
            anchor,
            provider_from_wire(&v2.harness.provider, HarnessProvider::Claude),
        ),
        class,
        text: payload.text,
        origin: TurnOrigin::Envelope {
            event_id: v2.event.id.clone(),
            seq: v2.event.seq,
        },
        model: v2.event.model.clone(),
    })
}
