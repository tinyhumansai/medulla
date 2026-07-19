//! The core runtime's in-memory data model: the per-thread state ([`Thread`]),
//! the connection-wide fold state ([`State`]), and the [`CoreRuntime`] adapter
//! itself, plus the caps and thresholds that bound them. Fields and helpers are
//! `pub(super)` so the sibling logic modules (connect, fold, driver) can drive
//! them; nothing here is exposed outside the `core` module tree.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

use crate::memory::MemoryService;
use crate::runtime::core_client::{CoreClient, SeqTracker};
use crate::runtime::{CycleResultSummary, ThreadSummary, WorkerInfo};
use crate::ui::chat_store::ChatMessage;
use crate::ui::events::{EventEnvelope, TuiEvent};

/// The upper bound on a thread's retained event log before the oldest are dropped.
const EVENT_CAP: usize = 5000;
/// The upper bound on a thread's retained chat-only event log.
const CHAT_CAP: usize = 2000;
/// A running cycle silent for longer than this reads as a stalled stream (spec §4).
pub(super) const STALL_MS: i64 = 8000;

/// One local thread over a core thread.
pub(super) struct Thread {
    pub(super) id: String,
    /// The durable core `threadId` (`th_…`); empty while a fork/session is being made.
    pub(super) core_id: String,
    pub(super) parent_id: Option<String>,
    pub(super) name: String,
    pub(super) messages: Vec<ChatMessage>,
    pub(super) events: Vec<EventEnvelope>,
    pub(super) chat_events: Vec<EventEnvelope>,
    pub(super) running: bool,
    pub(super) last_result: Option<CycleResultSummary>,
    pub(super) latest_cycle_id: Option<String>,
    pub(super) seq_tracker: SeqTracker,
}

impl Thread {
    /// Create a fresh local thread bound to `core_id` (which may be empty until a
    /// fork/session lands a durable `threadId`).
    pub(super) fn new(id: &str, name: &str, core_id: String) -> Self {
        Thread {
            id: id.to_string(),
            core_id,
            parent_id: None,
            name: name.to_string(),
            messages: Vec::new(),
            events: Vec::new(),
            chat_events: Vec::new(),
            running: false,
            last_result: None,
            latest_cycle_id: None,
            seq_tracker: SeqTracker::new(0),
        }
    }
}

/// The connection-wide fold state: every local thread plus the display counters and
/// stall bookkeeping shared across the fold loop, watchdog, and `Runtime` methods.
pub(super) struct State {
    pub(super) threads: Vec<Thread>,
    pub(super) active_id: String,
    pub(super) next_thread: usize,
    /// Local monotonic seq for every folded `EventEnvelope` (display only).
    pub(super) seq: u64,
    pub(super) workers: Vec<WorkerInfo>,
    pub(super) resyncing: bool,
    /// Wall-clock of the last folded event, for the stall guard.
    pub(super) last_event_at: i64,
    /// Silence threshold (ms) before a running cycle reads as stalled. Defaults to
    /// [`STALL_MS`]; a test seam ([`CoreRuntime::set_stall_ms`]) can shorten it.
    pub(super) stall_ms: i64,
    pub(super) async_mode: bool,
}

impl State {
    /// The active thread, by `active_id`. Panics if it has been removed (never happens).
    pub(super) fn active(&self) -> &Thread {
        self.threads
            .iter()
            .find(|t| t.id == self.active_id)
            .expect("active thread")
    }
    /// Find a thread by its local id.
    pub(super) fn by_id(&mut self, id: &str) -> Option<&mut Thread> {
        self.threads.iter_mut().find(|t| t.id == id)
    }
    /// Find a thread by its durable core `threadId`.
    pub(super) fn by_core(&mut self, core_id: &str) -> Option<&mut Thread> {
        self.threads.iter_mut().find(|t| t.core_id == core_id)
    }

    /// Append a folded event to `thread`, mirroring chatty events into its chat-only
    /// log and evicting the oldest once either log exceeds its cap.
    pub(super) fn push_event(thread: &mut Thread, env: EventEnvelope) {
        let chatty = matches!(
            env.event,
            TuiEvent::User { .. } | TuiEvent::Assistant { .. } | TuiEvent::Error { .. }
        );
        thread.events.push(env.clone());
        if thread.events.len() > EVENT_CAP {
            let drop = thread.events.len() - EVENT_CAP;
            thread.events.drain(0..drop);
        }
        if chatty {
            thread.chat_events.push(env);
            if thread.chat_events.len() > CHAT_CAP {
                let drop = thread.chat_events.len() - CHAT_CAP;
                thread.chat_events.drain(0..drop);
            }
        }
    }

    /// Project every thread into a [`ThreadSummary`] for the Chat-tab sidebar,
    /// counting still-running tasks and attention/error events per thread.
    pub(super) fn thread_summaries(&self) -> Vec<ThreadSummary> {
        self.threads
            .iter()
            .map(|t| {
                let mut running_tasks = 0i64;
                let mut attention = 0usize;
                for env in &t.events {
                    match &env.event {
                        TuiEvent::TaskStart { .. } => running_tasks += 1,
                        TuiEvent::TaskComplete { .. } => running_tasks -= 1,
                        TuiEvent::TaskAttention { .. } | TuiEvent::Error { .. } => attention += 1,
                        _ => {}
                    }
                }
                ThreadSummary {
                    id: t.id.clone(),
                    parent_id: t.parent_id.clone(),
                    name: t.name.clone(),
                    running: t.running,
                    turns: t.messages.len().div_ceil(2),
                    running_tasks: running_tasks.max(0) as usize,
                    attention,
                }
            })
            .collect()
    }

    /// Reset the active thread's local state (keeps its id, clears its core binding).
    pub(super) fn active_mut_reset(&mut self) -> &mut Thread {
        let id = self.active_id.clone();
        let t = self
            .threads
            .iter_mut()
            .find(|t| t.id == id)
            .expect("active thread");
        t.core_id.clear();
        t.messages.clear();
        t.events.clear();
        t.chat_events.clear();
        t.running = false;
        t.last_result = None;
        t.latest_cycle_id = None;
        t
    }
}

/// A [`Runtime`](crate::runtime::Runtime) over a connected [`CoreClient`].
pub struct CoreRuntime {
    pub(super) client: Arc<CoreClient>,
    pub(super) state: Arc<Mutex<State>>,
    pub(super) tx: broadcast::Sender<()>,
    /// Set once the connection drops, so the UI can stop treating the stream as live.
    pub(super) closed: Arc<AtomicBool>,
    /// The attached persona-memory service, when memory is enabled.
    pub(super) memory: Option<Arc<MemoryService>>,
}
