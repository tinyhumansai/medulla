//! Private state retained while folding one turn's progress stream.

use super::super::super::types::OnEvent;
use super::progress::ProgressFold;

/// Accumulating sink for the semantic events one turn produces.
///
/// Owns the caller's per-event callback, the transcript line counter, the count
/// of what was actually produced, and the stream fold — so they move together.
/// Every emission advances the line, bumps the count, folds the next delta, and
/// (when there is one) reaches the callback; a caller that could bump one
/// without the others is a caller that can report a count the transcript does
/// not support.
pub(super) struct EventSink {
    /// The caller's per-event callback, when it registered one.
    pub(super) on_event: Option<OnEvent>,
    /// Number of events emitted so far — what
    /// [`super::super::super::types::RunTaskResult::events`] reports.
    pub(super) emitted: usize,
    /// The stream fold that turns progress deltas into completed events.
    pub(super) fold: ProgressFold,
}

/// Per-turn accumulation of the deltas awaiting a phase boundary.
///
/// The core streams `TextDelta` and `ThinkingDelta` roughly once per token.
/// Emitting one event per token would exhaust the bounded transcript's entry
/// cap before the turn ended and hand the status throttler a per-token fragment
/// instead of the reasoning it treats as a cumulative snapshot, so the fold
/// defers them and emits whole — one `agent_message` per completed utterance,
/// one `agent_thinking` per reasoning block — at the next structural boundary.
#[derive(Default)]
pub(super) struct ProgressFold {
    /// Text deltas since the last boundary, emitted as one `agent_message`.
    pub(super) text: String,
    /// Thinking deltas since the last boundary, emitted once as the redacted,
    /// bounded reasoning snapshot.
    pub(super) thinking: String,
}
