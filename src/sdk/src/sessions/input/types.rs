//! Data types for the `input` module.
#[allow(unused_imports)]
use super::*;
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
