//! Data types for the `completion` module.
#[allow(unused_imports)]
use super::*;
/// A terminal `stop_reason` seen, held until its message is fully written.
///
/// Claude Code writes **one transcript record per content block**, repeating the
/// message-level `stop_reason` on every one. A final `[thinking, text]` message
/// therefore lands as two `end_turn` records — the thinking one first, carrying
/// no reply text at all. Settling on the first would answer with whatever
/// narration preceded it, or with nothing.
#[derive(Debug, Clone)]
pub(super) struct PendingTerminal {
    /// `message.id`, shared by every record of the same message. `None` when the
    /// record carried no id, in which case the next record of any kind closes it.
    pub(super) message_id: Option<String>,
    /// The `stop_reason` that ended the turn.
    pub(super) stop_reason: String,
}
/// What one transcript line said about the turn in flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnSignal {
    /// The assistant produced text but is not finished.
    Progress {
        /// The text of this record, for a live status line.
        text: String,
    },
    /// The assistant invoked a tool; it will continue.
    Tool {
        /// The tool's name.
        name: String,
    },
    /// The turn is over.
    Complete {
        /// Everything the assistant said this turn, oldest first.
        reply: String,
        /// The `stop_reason` that ended it.
        stop_reason: String,
    },
}
/// Folds transcript lines into turn-completion signals.
///
/// One watcher tracks one turn: construct it when a prompt is injected, feed it
/// every appended transcript line, and stop at the first
/// [`TurnSignal::Complete`].
#[derive(Debug, Clone)]
pub struct TurnWatcher {
    /// Which transcript dialect this watcher folds.
    pub(super) provider: HarnessProvider,
    /// Assistant text seen so far this turn, oldest first.
    pub(super) said: Vec<String>,
    /// Whether a tool call is outstanding — used only by the stall backstop, so
    /// a long build is never mistaken for a finished turn.
    pub(super) tool_outstanding: bool,
    /// A terminal record seen, waiting for the rest of its message.
    pub(super) pending: Option<PendingTerminal>,
    /// Whether the turn has already been settled.
    pub(super) done: bool,
}
