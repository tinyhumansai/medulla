//! [`TurnStream`] — the mode-independent half of running a turn.
//!
//! A harness emits the same information whichever way it is driven: semantic
//! events as it works, then a statement that the turn is over. Only two things
//! differ between the headless and interactive modes, and neither is the *data*:
//!
//! | | headless | interactive |
//! |---|---|---|
//! | where raw lines come from | the child's stdout | the harness's transcript file |
//! | what says "done" | a `result` frame on stdout | `end_turn` / `task_complete` in the transcript |
//!
//! So the fold between those two edges is shared, and lives here: raw JSONL line
//! in, [`HarnessSemanticEvent`]s out (for status frames), plus the reply once the
//! harness states it has finished. A caller supplies the lines; this decides what
//! they mean.
//!
//! Keeping it in one place is not tidiness. The progress events a peer sees while
//! its task runs are derived here, so a mode that folds its own lines is a mode
//! that silently reports differently — which is exactly the bug this module was
//! written to fix.

use crate::daemon::mappers::{HarnessLineMapper, HarnessSemanticEvent};
use crate::tinyplace::HarnessProvider;

use super::completion::{TurnSignal, TurnWatcher};

/// The outcome of folding one raw line.
#[derive(Debug, Clone)]
pub struct LineFold {
    /// Semantic events this line produced, in order. Drive status frames from
    /// these; they are identical in both modes.
    pub events: Vec<HarnessSemanticEvent>,
    /// The turn's answer, present only on the line that ended it.
    pub reply: Option<String>,
}

impl LineFold {
    /// Whether this line ended the turn.
    pub fn is_complete(&self) -> bool {
        self.reply.is_some()
    }
}

/// Folds a harness's raw output into semantic events and a completion.
///
/// One per turn. Feed it every line from whichever source the mode provides.
pub struct TurnStream {
    mapper: HarnessLineMapper,
    watcher: TurnWatcher,
    line_no: i64,
    events_seen: usize,
}

impl TurnStream {
    /// A stream for one turn on `provider`.
    pub fn new(provider: HarnessProvider) -> Self {
        TurnStream {
            mapper: HarnessLineMapper::new(provider.as_str()),
            watcher: TurnWatcher::for_provider(provider),
            line_no: 0,
            events_seen: 0,
        }
    }

    /// How many semantic events this turn has produced.
    pub fn events(&self) -> usize {
        self.events_seen
    }

    /// Whether the turn has ended.
    pub fn is_done(&self) -> bool {
        self.watcher.is_done()
    }

    /// Whether a tool call is outstanding — silence here means work, not an end.
    pub fn tool_outstanding(&self) -> bool {
        self.watcher.tool_outstanding()
    }

    /// Latest token usage the harness reported, if any.
    pub fn usage(&self) -> Option<crate::tinyplace::TokenUsage> {
        self.mapper.usage()
    }

    /// Fold one raw line.
    ///
    /// The mapper and the completion watcher both see it: the first yields the
    /// progress a peer is shown, the second decides whether the turn is over.
    /// They are deliberately independent — a line can carry progress, a
    /// completion, both, or neither.
    pub fn observe(&mut self, raw: &str) -> LineFold {
        if raw.trim().is_empty() {
            return LineFold {
                events: Vec::new(),
                reply: None,
            };
        }
        let events = self.mapper.map_line(raw, self.line_no);
        self.line_no += 1;
        self.events_seen += events.len();

        let reply = match self.watcher.observe(raw) {
            Some(TurnSignal::Complete { reply, .. }) => Some(reply),
            _ => None,
        };
        LineFold { events, reply }
    }

    /// Whether the turn should be given up on after `idle_ms` of silence.
    ///
    /// Refuses while a tool call is outstanding, so a long build is never
    /// mistaken for a finished turn.
    pub fn stalled_for(&self, idle_ms: i64, budget_ms: i64) -> bool {
        self.watcher.stalled_for(idle_ms, budget_ms)
    }

    /// Settle the turn from the stall backstop, returning whatever was said.
    pub fn settle_stalled(&mut self) -> String {
        match self.watcher.settle_stalled() {
            TurnSignal::Complete { reply, .. } => reply,
            _ => String::new(),
        }
    }
}
