//! Data model and trivial construction/mutation seams for the scripted mock
//! runtime.
//!
//! Holds the in-memory [`State`] (threads, roster, presence, sessions) and its
//! per-thread [`Thread`] records, the [`MockRuntime`] handle plus its scripted
//! [`ScriptedMemory`] surface, and the small helpers (id generation, event
//! emission, thread summarisation) shared by the behaviour submodules. The
//! `Runtime` trait impl lives in [`super::runtime_impl`] and the populated demo
//! scenario in [`super::scenario`]; both reach the internals here through
//! `pub(super)` items.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

use crate::runtime::{
    AgentDescriptor, AgentPresence, CycleResultSummary, PeerSession, ThreadSummary,
    TinyplaceIdentity, WorkerInfo,
};
use crate::ui::chat_store::ChatMessage;
use crate::ui::events::{EventEnvelope, TuiEvent};

/// Cap on retained events per thread before the oldest are dropped.
const EVENT_CAP: usize = 5000;
/// Cap on retained chat events per thread before the oldest are dropped.
const CHAT_CAP: usize = 2000;

/// One conversation thread: its chat transcript, event log, and run state.
pub(super) struct Thread {
    /// Stable thread id (e.g. `t1`).
    pub(super) id: String,
    /// Parent thread id when this thread was forked, else `None`.
    pub(super) parent_id: Option<String>,
    /// Human-facing thread name.
    pub(super) name: String,
    /// Session id assigned to this thread.
    pub(super) session_id: String,
    /// Chat messages exchanged in the thread.
    pub(super) messages: Vec<ChatMessage>,
    /// Full event log for the thread.
    pub(super) events: Vec<EventEnvelope>,
    /// Chat-only subset of `events` (user/assistant/error).
    pub(super) chat_events: Vec<EventEnvelope>,
    /// Whether a cycle is currently running in this thread.
    pub(super) running: bool,
    /// Summary of the last completed cycle, if any.
    pub(super) last_result: Option<CycleResultSummary>,
}

/// The whole scripted world: every thread plus shared roster/presence data.
pub(super) struct State {
    /// All threads, in creation order.
    pub(super) threads: Vec<Thread>,
    /// Id of the currently active thread.
    pub(super) active_id: String,
    /// Monotonic event sequence counter.
    seq: u64,
    /// Monotonic cycle counter used to mint cycle ids.
    pub(super) cycle_seq: u64,
    /// Whether async delegation mode is on.
    pub(super) async_mode: bool,
    /// Whether tracing is enabled.
    pub(super) tracing: bool,
    /// The scripted agent roster.
    pub(super) roster: Vec<AgentDescriptor>,
    /// The scripted worker registry, as `Runtime::workers` reports it. Distinct
    /// from `roster`: the registry is the fleet this process can delegate to,
    /// which is not necessarily what a backend advertises.
    pub(super) workers: Vec<WorkerInfo>,
    /// Presence keyed by agent id.
    pub(super) presence: HashMap<String, AgentPresence>,
    /// Peer sessions keyed by agent id.
    pub(super) sessions: HashMap<String, Vec<PeerSession>>,
    /// The tiny.place identity, when configured.
    pub(super) tinyplace: Option<TinyplaceIdentity>,
    /// Scripted agent-harness status, when a scenario exercises the harness
    /// task board. `None` by default so the Agents view degrades to nothing.
    pub(super) harness: Option<crate::harness_contract::HarnessStatus>,
}

impl State {
    /// Mutable handle to the active thread. Panics if it is missing (an invariant
    /// of the mock, which always keeps the active id pointing at a live thread).
    pub(super) fn active_mut(&mut self) -> &mut Thread {
        let id = self.active_id.clone();
        self.threads
            .iter_mut()
            .find(|t| t.id == id)
            .expect("active thread")
    }

    /// Shared handle to the active thread. Panics if it is missing.
    pub(super) fn active(&self) -> &Thread {
        self.threads
            .iter()
            .find(|t| t.id == self.active_id)
            .expect("active thread")
    }

    /// Append `event` to the active thread with a fresh sequence and timestamp,
    /// mirroring it into the chat log when it is a chat-visible event and
    /// trimming both logs to their caps.
    pub(super) fn emit(&mut self, event: TuiEvent) {
        self.seq += 1;
        let env = EventEnvelope {
            seq: self.seq,
            at: now_millis(),
            event,
        };
        let chatty = matches!(
            env.event,
            TuiEvent::User { .. } | TuiEvent::Assistant { .. } | TuiEvent::Error { .. }
        );
        let thread = self.active_mut();
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
}

/// Current wall-clock time in milliseconds, via the chat-store clock.
pub(super) fn now_millis() -> i64 {
    crate::ui::chat_store::now_millis()
}

/// Mint an id of the form `{prefix}-{millis}-{hex}` for sessions and threads.
pub(super) fn gen_id(prefix: &str) -> String {
    format!("{prefix}-{}-{:04x}", now_millis(), rand_suffix())
}

/// Cheap, dependency-free pseudo-random suffix derived from the clock.
fn rand_suffix() -> u16 {
    // Cheap, dependency-free pseudo-random from the clock.
    (now_millis() as u64)
        .wrapping_mul(2654435761)
        .rotate_left(13) as u16
}

/// A scripted runtime. Construct with [`MockRuntime::demo`] for a populated
/// snapshot or [`MockRuntime::empty`] for a bare one.
pub struct MockRuntime {
    /// The scripted world behind a mutex.
    pub(super) state: Arc<Mutex<State>>,
    /// Change-notification channel; every mutation pings it.
    pub(super) tx: broadcast::Sender<()>,
    /// Ordered log of runtime methods invoked (test seam).
    calls: Arc<Mutex<Vec<String>>>,
    /// Scripted persona-memory surface (test seam). `None` = no memory service.
    pub(super) memory: Arc<Mutex<Option<ScriptedMemory>>>,
    /// Scripted feedback board, mutated in place by votes and comments so the
    /// offline demo's controls behave like the real thing.
    pub(super) board: Arc<Mutex<super::feedback::MockBoard>>,
}

/// A scripted stand-in for a `MemoryService`, driven by tests via the
/// `set_memory_*` seams.
#[derive(Default, Clone)]
pub(super) struct ScriptedMemory {
    /// Scripted memory status, if attached.
    pub(super) status: Option<crate::memory::MemoryStatus>,
    /// Scripted search hits, returned (capped by `k`) from `memory_search`.
    pub(super) hits: Vec<crate::memory::MemoryHit>,
    /// Scripted persona directives.
    pub(super) directives: Vec<String>,
}

impl MockRuntime {
    /// Wrap a fully-built [`State`] into a runtime handle with fresh channels.
    fn from_state(state: State) -> Self {
        let (tx, _rx) = broadcast::channel(256);
        MockRuntime {
            state: Arc::new(Mutex::new(state)),
            tx,
            calls: Arc::new(Mutex::new(Vec::new())),
            memory: Arc::new(Mutex::new(None)),
            board: super::feedback::demo_board(),
        }
    }

    /// Attach a scripted memory status. Enables the mock's memory surface.
    pub fn set_memory_status(&self, status: crate::memory::MemoryStatus) {
        let mut guard = self.memory.lock().unwrap();
        guard.get_or_insert_with(ScriptedMemory::default).status = Some(status);
    }

    /// Script the hits returned by [`Runtime::memory_search`].
    pub fn set_memory_hits(&self, hits: Vec<crate::memory::MemoryHit>) {
        let mut guard = self.memory.lock().unwrap();
        guard.get_or_insert_with(ScriptedMemory::default).hits = hits;
    }

    /// Script the directives returned by [`Runtime::memory_directives`].
    pub fn set_memory_directives(&self, directives: Vec<String>) {
        let mut guard = self.memory.lock().unwrap();
        guard.get_or_insert_with(ScriptedMemory::default).directives = directives;
    }

    /// Record a runtime method invocation in the call log.
    pub(super) fn record(&self, name: &str) {
        self.calls.lock().unwrap().push(name.to_string());
    }

    /// The ordered log of runtime methods invoked on this mock. Test seam.
    pub fn recorded_calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    /// Emit an arbitrary event into the active thread and notify subscribers.
    /// Test/demo scripting seam.
    pub fn script_event(&self, event: TuiEvent) {
        {
            self.state.lock().unwrap().emit(event);
        }
        self.ping();
    }

    /// Script the registry returned by [`Runtime::workers`](crate::runtime::Runtime::workers).
    ///
    /// The worker registry is a separate surface from the snapshot roster — a
    /// locally-added tiny.place worker is in the former and not the latter —
    /// so views that read both need a mock that can populate them apart.
    pub fn set_workers(&self, workers: Vec<WorkerInfo>) {
        {
            self.state.lock().unwrap().workers = workers;
        }
        self.ping();
    }

    /// Force the active thread's running flag. Test/demo scripting seam.
    pub fn set_running(&self, running: bool) {
        {
            self.state.lock().unwrap().active_mut().running = running;
        }
        self.ping();
    }

    /// A bare runtime: one empty main thread, no roster.
    pub fn empty() -> Self {
        let session_id = gen_id("tui");
        let state = State {
            threads: vec![Thread {
                id: "t1".into(),
                parent_id: None,
                name: "main".into(),
                session_id,
                messages: Vec::new(),
                events: Vec::new(),
                chat_events: Vec::new(),
                running: false,
                last_result: None,
            }],
            active_id: "t1".into(),
            seq: 0,
            cycle_seq: 0,
            async_mode: false,
            tracing: false,
            roster: Vec::new(),
            workers: Vec::new(),
            presence: HashMap::new(),
            sessions: HashMap::new(),
            tinyplace: None,
            harness: None,
        };
        MockRuntime::from_state(state)
    }

    /// Notify subscribers that state changed.
    pub(super) fn ping(&self) {
        let _ = self.tx.send(());
    }

    /// Fold each thread's event log into a [`ThreadSummary`] (turn count, open
    /// tasks, and items needing attention) for the snapshot.
    pub(super) fn thread_summaries(state: &State) -> Vec<ThreadSummary> {
        state
            .threads
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
}
