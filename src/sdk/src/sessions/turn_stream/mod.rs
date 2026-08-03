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
use crate::protocol::HarnessProvider;

use super::completion::{TurnSignal, TurnWatcher};

#[cfg(test)]
mod tests;

impl LineFold {
    /// Whether this line ended the turn.
    pub fn is_complete(&self) -> bool {
        self.reply.is_some()
    }
}

impl TurnStream {
    /// A stream for one turn on `provider`.
    pub fn new(provider: HarnessProvider) -> Self {
        Self::new_with_gh_repo_override(provider, std::env::var_os("GH_REPO").is_some())
    }

    /// Build a turn stream with explicit effective child `GH_REPO` state.
    pub fn new_with_gh_repo_override(provider: HarnessProvider, gh_repo_is_set: bool) -> Self {
        TurnStream {
            mapper: HarnessLineMapper::new_with_gh_repo_override(provider.as_str(), gh_repo_is_set),
            watcher: TurnWatcher::for_provider(provider),
            line_no: 0,
            events_seen: 0,
        }
    }

    /// Seed checkout context retained by a previous turn in the same PTY.
    pub fn set_workspace_context(
        &mut self,
        cwd: Option<String>,
        branch: Option<String>,
        pull_request: Option<String>,
    ) {
        self.mapper.set_workspace_context(cwd, branch, pull_request);
    }

    /// Checkout context learned while folding this turn.
    pub fn workspace_context(&self) -> (Option<String>, Option<String>, Option<String>) {
        self.mapper.workspace_context()
    }

    /// Emit retained repository facts into a new task's otherwise-empty fold.
    pub fn retained_workspace_event(&mut self) -> Option<HarnessSemanticEvent> {
        let (cwd, branch, pull_request) = self.workspace_context();
        if cwd.is_none() && branch.is_none() && pull_request.is_none() {
            return None;
        }
        let mut payload = serde_json::Map::new();
        for (name, value) in [
            ("cwd", cwd),
            ("branch", branch),
            ("pull_request", pull_request),
        ] {
            if let Some(value) = value {
                payload.insert(name.to_string(), serde_json::Value::String(value));
            }
        }
        self.events_seen += 1;
        Some(HarnessSemanticEvent {
            line: self.line_no,
            timestamp_ms: crate::clock::now_millis(),
            record_type: "retained:workspace".to_string(),
            event: crate::protocol::HarnessEvent {
                kind: crate::harness_work::kinds::SESSION_INFO.to_string(),
                payload: serde_json::Value::Object(payload),
                ..Default::default()
            },
        })
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
    pub fn usage(&self) -> Option<crate::protocol::TokenUsage> {
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

    /// Whether the turn has ended but its reply may still be being written.
    ///
    /// See [`TurnWatcher::terminal_pending`](crate::sessions::TurnWatcher::terminal_pending).
    pub fn terminal_pending(&self) -> bool {
        self.watcher.terminal_pending()
    }

    /// Close a pending terminal, returning the reply. `None` if none is pending.
    pub fn settle_pending(&mut self) -> Option<String> {
        match self.watcher.settle_pending() {
            Some(TurnSignal::Complete { reply, .. }) => Some(reply),
            _ => None,
        }
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

mod types;
pub use types::LineFold;
pub use types::TurnStream;
