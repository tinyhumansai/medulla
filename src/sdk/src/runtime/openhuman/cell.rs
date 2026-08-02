//! The folded snapshot plus its change-notification channel.
//!
//! Split out from the runtime so the render contract — snapshot in, ping out —
//! can be exercised without booting a core. The core uses process globals (a
//! `OnceLock` context, a singleton event bus) and cannot be stood up per test,
//! so anything that genuinely needs one belongs in an integration test. Keeping
//! this piece core-free is what makes the contract unit-testable at all.

use std::sync::Mutex;

use tokio::sync::broadcast;

use crate::runtime::types::{AgentDescriptor, RuntimeSnapshot, ThreadSummary};
use crate::ui::events::EventEnvelope;

/// Holds the folded view and notifies readers when it changes.
pub struct SnapshotCell {
    /// A plain mutex, not an async one: every write is a whole-snapshot swap,
    /// so the lock is never held across an await and there is no partial state
    /// for a reader to observe.
    state: Mutex<RuntimeSnapshot>,
    /// Payload-free ping. The contract is "something moved, re-read the
    /// snapshot", which keeps a slow reader from having to replay a backlog and
    /// makes a lagging receiver harmless.
    tx: broadcast::Sender<()>,
}

impl SnapshotCell {
    /// An empty cell.
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(16);
        Self {
            state: Mutex::new(RuntimeSnapshot::default()),
            tx,
        }
    }

    /// The current folded view.
    pub fn snapshot(&self) -> RuntimeSnapshot {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Subscribe to change notifications.
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.tx.subscribe()
    }

    /// Replace the roster and thread list, then notify.
    pub fn apply(&self, roster: Vec<AgentDescriptor>, threads: Vec<ThreadSummary>) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.roster = roster;
            state.threads = threads;
        }
        // An absent or lagging receiver is not an error — the ping is advisory
        // and the next `snapshot()` reads the same state regardless.
        let _ = self.tx.send(());
    }

    /// Append newly replayed events and notify.
    ///
    /// Appends rather than replaces: events are a growing log, and the caller
    /// only ever fetches what is past its cursor. De-duplication is the
    /// cursor's job, not this method's — re-applying an old batch here would
    /// duplicate rows rather than being idempotent.
    ///
    /// A `running` flag rides along because it is derived from the same fetch:
    /// the caller knows whether the turn settled, and splitting it into a
    /// second call would let the two disagree between locks.
    ///
    /// `None` means "the fetch said nothing about liveness" and leaves the flag
    /// alone. Most batches are mid-turn output that carries no cycle boundary,
    /// and forcing a `bool` there would make every such batch re-assert
    /// `running = true` — which is how a settled turn ends up spinning forever
    /// once a late event arrives behind its `cycle_end`.
    pub fn append_events(&self, events: Vec<EventEnvelope>, running: Option<bool>) {
        if events.is_empty() {
            // Still record liveness — a turn can settle without new events.
            let Some(running) = running else {
                return;
            };
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.running == running {
                return;
            }
            state.running = running;
        } else {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.events.extend(events.iter().cloned());
            // The chat view wants only conversational rows; everything else is
            // trace. Splitting here keeps the render layer from re-filtering
            // the whole log on every frame.
            state
                .chat_events
                .extend(events.into_iter().filter(is_chat_row));
            if let Some(running) = running {
                state.running = running;
            }
        }
        let _ = self.tx.send(());
    }

    /// The thread currently selected, or empty when none has been.
    pub fn active_thread_id(&self) -> String {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active_thread_id
            .clone()
    }

    /// Record the operator's active thread.
    pub fn set_active_thread(&self, id: String) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.active_thread_id = id;
    }

    /// Point the snapshot at a different thread and drop the previous one's log.
    ///
    /// The transcript is per-thread, so keeping the old rows would render two
    /// conversations as one — and the incoming thread replays from its own
    /// start, which would interleave them by arrival rather than by thread.
    /// `running` resets too: the previous thread's liveness says nothing about
    /// this one, and the first batch re-establishes it.
    pub fn switch_thread(&self, id: String) {
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.active_thread_id = id;
            state.events.clear();
            state.chat_events.clear();
            state.running = false;
        }
        let _ = self.tx.send(());
    }
}

impl Default for SnapshotCell {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether an event belongs in the chat transcript rather than the trace.
///
/// Deliberately a small allow-list rather than a deny-list: a new event kind
/// added upstream should default to *trace*, where an unexpected row is noise,
/// not to the transcript, where it would read as something the user or the
/// assistant said.
fn is_chat_row(env: &EventEnvelope) -> bool {
    use crate::ui::events::TuiEvent;
    matches!(
        env.event,
        TuiEvent::User { .. } | TuiEvent::Assistant { .. } | TuiEvent::AssistantDelta { .. }
    )
}
