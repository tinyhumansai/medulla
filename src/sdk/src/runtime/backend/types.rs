//! The backend runtime's local data model: one [`Thread`] per backend session,
//! the aggregate [`State`] over all threads, and the [`BackendRuntime`] handle
//! the UI holds. Only the struct definitions and their trivial accessors live
//! here; the behaviour-heavy event fold is in [`fold`](super::fold), the SSE
//! wiring in [`stream`](super::stream), and the [`Runtime`](crate::runtime::Runtime)
//! trait surface in [`runtime`](super::runtime).

use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::client::MedullaClient;
use crate::runtime::CycleResultSummary;
use crate::ui::chat_store::ChatMessage;
use crate::ui::events::EventEnvelope;

/// Upper bound on the per-thread raw event log; older events are dropped.
pub(super) const EVENT_CAP: usize = 5000;
/// Upper bound on the per-thread chat-events subset; older events are dropped.
pub(super) const CHAT_CAP: usize = 2000;

/// One local thread over a backend session.
pub(super) struct Thread {
    /// Stable local thread id (e.g. `t1`).
    pub(super) id: String,
    /// The thread this one was forked from, if any.
    pub(super) parent_id: Option<String>,
    /// Human-facing thread name.
    pub(super) name: String,
    /// Backend session id; empty while a session is being created.
    pub(super) session_id: String,
    /// Rendered user/assistant transcript for this thread.
    pub(super) messages: Vec<ChatMessage>,
    /// The full folded event log, capped at [`EVENT_CAP`].
    pub(super) events: Vec<EventEnvelope>,
    /// The user/assistant/error subset, capped at [`CHAT_CAP`].
    pub(super) chat_events: Vec<EventEnvelope>,
    /// Whether a cycle is currently running on this thread.
    pub(super) running: bool,
    /// Summary of the most recently completed cycle, if any.
    pub(super) last_result: Option<CycleResultSummary>,
    /// A user message appended optimistically on submit, awaiting its echo from
    /// the stream so the folded echo can be de-duplicated.
    pub(super) pending_user_echo: Option<String>,
    /// The SSE task folding this thread's session, if attached.
    pub(super) stream_task: Option<JoinHandle<()>>,
}

impl Thread {
    /// Create an empty thread bound to `session_id` (which may be empty until a
    /// backend session is created).
    pub(super) fn new(id: &str, name: &str, session_id: String) -> Self {
        Thread {
            id: id.to_string(),
            parent_id: None,
            name: name.to_string(),
            session_id,
            messages: Vec::new(),
            events: Vec::new(),
            chat_events: Vec::new(),
            running: false,
            last_result: None,
            pending_user_echo: None,
            stream_task: None,
        }
    }

    /// Abort any attached stream task and clear all folded transcript state,
    /// leaving the thread's identity (id/name/parent) intact.
    pub(super) fn reset(&mut self) {
        if let Some(h) = self.stream_task.take() {
            h.abort();
        }
        self.messages.clear();
        self.events.clear();
        self.chat_events.clear();
        self.running = false;
        self.last_result = None;
        self.pending_user_echo = None;
    }
}

/// The aggregate state over every local thread, shared behind an `Arc<Mutex<_>>`.
pub(super) struct State {
    /// Every open thread.
    pub(super) threads: Vec<Thread>,
    /// The id of the thread the UI is currently viewing.
    pub(super) active_id: String,
    /// Monotonic local sequence counter assigned to every folded event.
    pub(super) seq: u64,
    /// The next numeric suffix to hand out when forking a thread.
    pub(super) next_thread: usize,
    /// Local-only async toggle; see the module doc for why it is inert.
    pub(super) async_mode: bool,
}

impl State {
    /// The currently active thread. Panics if the active id has no thread, which
    /// is an invariant violation.
    pub(super) fn active(&self) -> &Thread {
        self.threads
            .iter()
            .find(|t| t.id == self.active_id)
            .expect("active thread")
    }

    /// The currently active thread, mutably. Panics on the same invariant as
    /// [`active`](State::active).
    pub(super) fn active_mut(&mut self) -> &mut Thread {
        let id = self.active_id.clone();
        self.threads
            .iter_mut()
            .find(|t| t.id == id)
            .expect("active thread")
    }

    /// Find a thread by its local id.
    pub(super) fn by_id(&mut self, id: &str) -> Option<&mut Thread> {
        self.threads.iter_mut().find(|t| t.id == id)
    }

    /// Find a thread by its backend session id.
    pub(super) fn by_session(&mut self, session_id: &str) -> Option<&mut Thread> {
        self.threads.iter_mut().find(|t| t.session_id == session_id)
    }
}

/// A [`Runtime`](crate::runtime::Runtime) over a live [`MedullaClient`].
pub struct BackendRuntime {
    /// The HTTP + SSE client used for every backend call.
    pub(super) client: MedullaClient,
    /// Shared local thread state folded from the streams.
    pub(super) state: Arc<Mutex<State>>,
    /// Broadcast handle pinged after every fold so the UI re-pulls a snapshot.
    pub(super) tx: broadcast::Sender<()>,
    /// Live orchestrator-hub roster control, filled after the hub connects. When
    /// present, `workers()`/`worker_op()` manage the hub's tiny.place peers; an
    /// empty slot means no worker surface (the default).
    pub(super) hub: Arc<Mutex<Option<crate::hub::HubHandle>>>,
}
