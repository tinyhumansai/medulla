//! A [`Runtime`] backed by the core-js orchestration core over its NDJSON Unix
//! socket ([`CoreClient`]). This is the second concrete runtime alongside
//! [`BackendRuntime`](crate::runtime::backend) (HTTP/SSE) and
//! [`MockRuntime`](crate::runtime::mock).
//!
//! Threads map to core threads (`thread.list`/`create`/`resume`/`fork`), each with a
//! `thread.subscribe` tap. One connection-wide event receiver funnels every frame;
//! the fold loop routes it by `threadId` and folds it into that thread's event log,
//! applying the §3.3 normalizations in [`map_core_event`]:
//!
//!   - `task_complete` is flat on the wire — rebuilt into the TUI's nested `TaskDigest`,
//!   - the envelope `cycleId` is folded into the lane key (`<cycleId>/t:<taskId>`) so
//!     two cycles delegating the same bare `taskId` never collide (§3.3(2)/§4.4),
//!   - `cancelled` stays distinct from `failed` (handled in `agents.rs`),
//!   - a `task_complete` with no `task_start` still lands a lane (handled in `agents.rs`).
//!
//! A `seq` gap (§3.2) triggers a `snapshot.get` resync: the thread's event log is
//! rebuilt from the durable folded snapshot and a status note is emitted.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::anyhow;
use futures::future::BoxFuture;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};

use crate::runtime::core_client::{CoreClient, CoreEvent, SeqTracker};
use crate::runtime::{
    AgentDescriptor, ContextItem, CycleResultSummary, Runtime, RuntimeSnapshot, StreamState,
    ThreadSummary, TinyplaceIdentity, WorkerInfo, WorkerOp,
};
use crate::ui::chat_store::{now_millis, ChatMessage, MainChatSummary};
use crate::ui::events::{EventEnvelope, TaskDigest, TuiEvent, Usage};

const EVENT_CAP: usize = 5000;
const CHAT_CAP: usize = 2000;
/// A running cycle silent for longer than this reads as a stalled stream (spec §4).
const STALL_MS: i64 = 8000;

/// Compose the lane-unique task key from the envelope `cycleId` and the wire `taskId`
/// (§3.3(2)/§4.4), mirroring the library's `taskCycleId` and the core's `store.taskKey`.
fn compose_task_id(cycle_id: &str, task_id: &str) -> String {
    if cycle_id.is_empty() {
        task_id.to_string()
    } else {
        format!("{cycle_id}/t:{task_id}")
    }
}

fn opt_str(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Map a core event body `{kind, ...}` onto the TUI's [`TuiEvent`], applying the §3.3
/// normalizations. `cycle_id` comes from the envelope (§3.2).
pub fn map_core_event(body: &Value, cycle_id: &str) -> TuiEvent {
    let kind = body.get("kind").and_then(Value::as_str).unwrap_or("");
    let s = |k: &str| {
        body.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let i = |k: &str| body.get(k).and_then(Value::as_i64).unwrap_or(0);
    match kind {
        "task_start" => TuiEvent::TaskStart {
            task_id: compose_task_id(cycle_id, &s("taskId")),
            instruction: s("instruction"),
            depth: i("depth"),
            agent_id: opt_str(body, "agentId"),
        },
        "task_event" => TuiEvent::TaskEvent {
            task_id: compose_task_id(cycle_id, &s("taskId")),
            event_kind: s("eventKind"),
            content: s("content"),
            harness: opt_str(body, "harness"),
        },
        "task_attention" => TuiEvent::TaskAttention {
            task_id: compose_task_id(cycle_id, &s("taskId")),
            reason: s("reason"),
            content: s("content"),
            question_id: opt_str(body, "questionId"),
        },
        "task_complete" => {
            // §3.3(1): the wire body is already flat — status/digest sit at the top
            // level, not under `digest`. Rebuild the TUI's nested `TaskDigest`.
            let status = {
                let raw = s("status");
                if raw.is_empty() {
                    "done".into()
                } else {
                    raw
                }
            };
            let usage = body
                .get("usage")
                .and_then(|u| serde_json::from_value::<Usage>(u.clone()).ok());
            TuiEvent::TaskComplete {
                digest: TaskDigest {
                    task_id: compose_task_id(cycle_id, &s("taskId")),
                    status,
                    digest: s("digest"),
                    result_ref: body.get("resultRef").cloned(),
                    usage,
                    depth: i("depth"),
                },
            }
        }
        // Everything else deserializes straight through the TuiEvent vocabulary; an
        // unknown kind rides through as `Unknown` rather than being dropped.
        _ => {
            serde_json::from_value::<TuiEvent>(body.clone()).unwrap_or_else(|_| TuiEvent::Unknown {
                kind: kind.to_string(),
                data: body.as_object().cloned().unwrap_or_default(),
            })
        }
    }
}

/// Fold a subscribe / `snapshot.get` snapshot's `{tasks[], chat[]}` into a replayable
/// event log (§3.4). Each folded task becomes a `task_start` (+ a `task_event` from
/// its `lastEvent`, + a `task_complete` when terminal); each chat entry a user /
/// assistant turn. The synthetic seqs start at `*seq` and advance it.
fn synth_from_snapshot(snapshot: &Value, seq: &mut u64) -> Vec<EventEnvelope> {
    let mut out = Vec::new();
    let at = snapshot
        .get("at")
        .and_then(Value::as_i64)
        .unwrap_or_else(now_millis);
    let mut push = |seq: &mut u64, event: TuiEvent| {
        *seq += 1;
        out.push(EventEnvelope {
            seq: *seq,
            at,
            event,
        });
    };
    if let Some(chat) = snapshot.get("chat").and_then(Value::as_array) {
        for c in chat {
            let body = c
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            match c.get("role").and_then(Value::as_str) {
                Some("user") => push(seq, TuiEvent::User { body }),
                Some("assistant") => push(seq, TuiEvent::Assistant { body }),
                _ => {}
            }
        }
    }
    if let Some(tasks) = snapshot.get("tasks").and_then(Value::as_array) {
        for t in tasks {
            let cycle_id = t.get("cycleId").and_then(Value::as_str).unwrap_or("");
            let task_id = compose_task_id(
                cycle_id,
                t.get("taskId").and_then(Value::as_str).unwrap_or(""),
            );
            push(
                seq,
                TuiEvent::TaskStart {
                    task_id: task_id.clone(),
                    instruction: t
                        .get("instruction")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    depth: t.get("depth").and_then(Value::as_i64).unwrap_or(0),
                    agent_id: opt_str(t, "agentId"),
                },
            );
            if let Some(le) = t.get("lastEvent") {
                push(
                    seq,
                    TuiEvent::TaskEvent {
                        task_id: task_id.clone(),
                        event_kind: le
                            .get("eventKind")
                            .and_then(Value::as_str)
                            .unwrap_or("status")
                            .to_string(),
                        content: le
                            .get("content")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        harness: opt_str(t, "harness"),
                    },
                );
            }
            let status = t.get("status").and_then(Value::as_str).unwrap_or("running");
            if status != "running" {
                let usage = t
                    .get("usage")
                    .and_then(|u| serde_json::from_value::<Usage>(u.clone()).ok());
                push(
                    seq,
                    TuiEvent::TaskComplete {
                        digest: TaskDigest {
                            task_id,
                            status: status.to_string(),
                            digest: t
                                .get("digest")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                            result_ref: None,
                            usage,
                            depth: t.get("depth").and_then(Value::as_i64).unwrap_or(0),
                        },
                    },
                );
            }
        }
    }
    out
}

/// One local thread over a core thread.
struct Thread {
    id: String,
    /// The durable core `threadId` (`th_…`); empty while a fork/session is being made.
    core_id: String,
    parent_id: Option<String>,
    name: String,
    messages: Vec<ChatMessage>,
    events: Vec<EventEnvelope>,
    chat_events: Vec<EventEnvelope>,
    running: bool,
    last_result: Option<CycleResultSummary>,
    latest_cycle_id: Option<String>,
    seq_tracker: SeqTracker,
}

impl Thread {
    fn new(id: &str, name: &str, core_id: String) -> Self {
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

struct State {
    threads: Vec<Thread>,
    active_id: String,
    next_thread: usize,
    /// Local monotonic seq for every folded `EventEnvelope` (display only).
    seq: u64,
    workers: Vec<WorkerInfo>,
    resyncing: bool,
    /// Wall-clock of the last folded event, for the stall guard.
    last_event_at: i64,
    /// Silence threshold (ms) before a running cycle reads as stalled. Defaults to
    /// [`STALL_MS`]; a test seam ([`CoreRuntime::set_stall_ms`]) can shorten it.
    stall_ms: i64,
    async_mode: bool,
}

impl State {
    fn active(&self) -> &Thread {
        self.threads
            .iter()
            .find(|t| t.id == self.active_id)
            .expect("active thread")
    }
    fn by_id(&mut self, id: &str) -> Option<&mut Thread> {
        self.threads.iter_mut().find(|t| t.id == id)
    }
    fn by_core(&mut self, core_id: &str) -> Option<&mut Thread> {
        self.threads.iter_mut().find(|t| t.core_id == core_id)
    }

    fn push_event(thread: &mut Thread, env: EventEnvelope) {
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

    fn thread_summaries(&self) -> Vec<ThreadSummary> {
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
}

/// Parse a `worker.list`-shaped payload `{workers: [...], selectedId}` into rows.
fn workers_from_payload(payload: &Value) -> Vec<WorkerInfo> {
    payload
        .get("workers")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|w| {
                    let id = w.get("id").and_then(Value::as_str)?.to_string();
                    let address = w.get("address").and_then(Value::as_str)?.to_string();
                    Some(WorkerInfo {
                        id,
                        address,
                        handle: opt_str(w, "handle"),
                        label: opt_str(w, "label"),
                        harness: opt_str(w, "harness"),
                        peer_id: opt_str(w, "peerId"),
                        selected: w.get("selected").and_then(Value::as_bool).unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A [`Runtime`] over a connected [`CoreClient`].
pub struct CoreRuntime {
    client: Arc<CoreClient>,
    state: Arc<Mutex<State>>,
    tx: broadcast::Sender<()>,
    /// Set once the connection drops, so the UI can stop treating the stream as live.
    closed: Arc<AtomicBool>,
}

impl CoreRuntime {
    /// Connect: handshake, adopt (or create) an active thread, subscribe, seed its
    /// snapshot, then spawn the fold loop and a stall watchdog.
    pub async fn connect(
        client: CoreClient,
        events_rx: mpsc::UnboundedReceiver<CoreEvent>,
        client_version: &str,
    ) -> anyhow::Result<Self> {
        client
            .initialize(client_version)
            .await
            .map_err(|e| anyhow!("core handshake failed: {e}"))?;

        // Adopt the first existing thread, or create one.
        let listed = client
            .thread_list()
            .await
            .map_err(|e| anyhow!(e.to_string()))?;
        let core_id = listed
            .get("threads")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(|t| t.get("threadId"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let core_id = match core_id {
            Some(id) => id,
            None => client
                .thread_create(Some("main"), Some("app"))
                .await
                .map_err(|e| anyhow!(e.to_string()))?,
        };

        let mut state = State {
            threads: vec![Thread::new("t1", "main", core_id.clone())],
            active_id: "t1".into(),
            next_thread: 2,
            seq: 0,
            workers: Vec::new(),
            resyncing: false,
            last_event_at: now_millis(),
            stall_ms: STALL_MS,
            async_mode: false,
        };

        // Subscribe + seed the active thread's snapshot before any live event lands.
        let sub = client
            .thread_subscribe(&core_id, None)
            .await
            .map_err(|e| anyhow!(e.to_string()))?;
        let baseline = sub.get("baselineSeq").and_then(Value::as_u64).unwrap_or(0);
        if let Some(snapshot) = sub.get("snapshot") {
            let synth = synth_from_snapshot(snapshot, &mut state.seq);
            let t = &mut state.threads[0];
            for env in synth {
                if let TuiEvent::User { body } | TuiEvent::Assistant { body } = &env.event {
                    let role = if matches!(env.event, TuiEvent::User { .. }) {
                        "user"
                    } else {
                        "assistant"
                    };
                    t.messages.push(ChatMessage {
                        role: role.into(),
                        content: body.clone(),
                    });
                }
                State::push_event(t, env);
            }
        }
        state.threads[0].seq_tracker = SeqTracker::new(baseline);

        // Best-effort worker registry seed (a core with no worker surface just errors).
        if let Ok(list) = client.worker_list().await {
            state.workers = workers_from_payload(&list);
        }

        let (tx, _rx) = broadcast::channel(256);
        let rt = CoreRuntime {
            client: Arc::new(client),
            state: Arc::new(Mutex::new(state)),
            tx,
            closed: Arc::new(AtomicBool::new(false)),
        };

        rt.spawn_fold_loop(events_rx);
        rt.spawn_watchdog();
        Ok(rt)
    }

    fn ping(&self) {
        let _ = self.tx.send(());
    }

    /// Test seam: shorten the stall-detection threshold (ms). No behavior change at
    /// the [`STALL_MS`] default; exists so tests can exercise the `Stalled` state
    /// without waiting out the production silence window.
    #[doc(hidden)]
    pub fn set_stall_ms(&self, ms: i64) {
        self.state.lock().unwrap().stall_ms = ms;
    }

    /// The connection-wide fold loop: route each event to its thread, detect `seq`
    /// gaps, and rebuild from a `snapshot.get` on a gap.
    fn spawn_fold_loop(&self, mut events_rx: mpsc::UnboundedReceiver<CoreEvent>) {
        let client = self.client.clone();
        let state = self.state.clone();
        let tx = self.tx.clone();
        let closed = self.closed.clone();
        tokio::spawn(async move {
            while let Some(ev) = events_rx.recv().await {
                // Detect a gap under the thread's tracker before folding.
                let (gap, resync_from, core_id) = {
                    let mut s = state.lock().unwrap();
                    match s.by_core(&ev.thread_id) {
                        Some(t) => {
                            let from = t.seq_tracker.last_seq();
                            let gap = t.seq_tracker.observe(ev.seq);
                            (gap, from, ev.thread_id.clone())
                        }
                        None => (false, 0, String::new()),
                    }
                };
                if core_id.is_empty() {
                    continue; // an event for a thread we do not track
                }

                if gap {
                    {
                        let mut s = state.lock().unwrap();
                        s.resyncing = true;
                    }
                    let _ = tx.send(());
                    if let Ok(payload) = client.snapshot_get(&core_id, Some(resync_from)).await {
                        let snapshot = payload.get("snapshot").cloned().unwrap_or(payload);
                        let mut s = state.lock().unwrap();
                        let base = s.seq;
                        let mut seq = base;
                        let synth = synth_from_snapshot(&snapshot, &mut seq);
                        s.seq = seq;
                        if let Some(t) = s.by_core(&core_id) {
                            t.events.clear();
                            t.chat_events.clear();
                            t.messages.clear();
                            for env in synth {
                                if let TuiEvent::User { body } | TuiEvent::Assistant { body } =
                                    &env.event
                                {
                                    let role = if matches!(env.event, TuiEvent::User { .. }) {
                                        "user"
                                    } else {
                                        "assistant"
                                    };
                                    t.messages.push(ChatMessage {
                                        role: role.into(),
                                        content: body.clone(),
                                    });
                                }
                                State::push_event(t, env);
                            }
                            // A visible status note that a gap was reconciled (§3.2).
                            s.seq += 1;
                            let seq = s.seq;
                            let note = EventEnvelope {
                                seq,
                                at: now_millis(),
                                event: TuiEvent::Effect {
                                    effect: json!({
                                        "kind": "resync",
                                        "note": format!("stream resynced from snapshot (seq gap after {resync_from})"),
                                    }),
                                },
                            };
                            if let Some(t) = s.by_core(&core_id) {
                                State::push_event(t, note);
                            }
                        }
                        s.resyncing = false;
                    }
                    let _ = tx.send(());
                }

                // Fold the live event itself.
                {
                    let mut s = state.lock().unwrap();
                    s.last_event_at = now_millis();
                    s.resyncing = false;
                    s.seq += 1;
                    let seq = s.seq;
                    let event = map_core_event(&ev.event, &ev.cycle_id);
                    if !ev.cycle_id.is_empty() {
                        if let Some(t) = s.by_core(&ev.thread_id) {
                            t.latest_cycle_id = Some(ev.cycle_id.clone());
                        }
                    }
                    if let Some(t) = s.by_core(&ev.thread_id) {
                        match &event {
                            TuiEvent::User { body } => t.messages.push(ChatMessage {
                                role: "user".into(),
                                content: body.clone(),
                            }),
                            TuiEvent::Assistant { body } => t.messages.push(ChatMessage {
                                role: "assistant".into(),
                                content: body.clone(),
                            }),
                            TuiEvent::CycleStart { .. } => t.running = true,
                            TuiEvent::CycleEnd { pass_count, .. } => {
                                t.running = false;
                                t.last_result = Some(CycleResultSummary {
                                    pass_count: *pass_count,
                                    task_ledger: Default::default(),
                                });
                            }
                            _ => {}
                        }
                        State::push_event(
                            t,
                            EventEnvelope {
                                seq,
                                at: ev.at,
                                event,
                            },
                        );
                    }
                }
                let _ = tx.send(());
            }
            closed.store(true, Ordering::Relaxed);
            let _ = tx.send(());
        });
    }

    /// A watchdog that pings ~every second while a cycle runs, so the UI re-pulls a
    /// snapshot and the stall indicator escalates even when the stream has gone silent.
    fn spawn_watchdog(&self) {
        let state = self.state.clone();
        let tx = self.tx.clone();
        let closed = self.closed.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(1000));
            loop {
                tick.tick().await;
                if closed.load(Ordering::Relaxed) {
                    break;
                }
                let running = { state.lock().unwrap().active().running };
                if running {
                    let _ = tx.send(());
                }
            }
        });
    }
}

impl Runtime for CoreRuntime {
    fn snapshot(&self) -> RuntimeSnapshot {
        let s = self.state.lock().unwrap();
        let threads = s.thread_summaries();
        let active = s.active();
        RuntimeSnapshot {
            session_id: active.core_id.clone(),
            running: active.running,
            events: active.events.clone(),
            chat_events: active.chat_events.clone(),
            messages: active.messages.clone(),
            last_result: active.last_result.clone(),
            tracing: false,
            roster: Vec::<AgentDescriptor>::new(),
            presence: Default::default(),
            sessions: Default::default(),
            tinyplace: None::<TinyplaceIdentity>,
            async_mode: s.async_mode,
            threads,
            active_thread_id: s.active_id.clone(),
        }
    }

    fn subscribe(&self) -> broadcast::Receiver<()> {
        self.tx.subscribe()
    }

    fn submit(&self, input: String) -> BoxFuture<'static, anyhow::Result<()>> {
        let client = self.client.clone();
        let state = self.state.clone();
        let tx = self.tx.clone();
        Box::pin(async move {
            let (core_id, thread_id) = {
                let s = state.lock().unwrap();
                let t = s.active();
                if t.core_id.is_empty() {
                    return Err(anyhow!("thread is still being created; try again"));
                }
                if t.running {
                    return Err(anyhow!("a cycle is already running"));
                }
                (t.core_id.clone(), t.id.clone())
            };
            // Optimistically mark running so the UI shows working immediately; the
            // stream's cycle_start/cycle_end are authoritative.
            {
                let mut s = state.lock().unwrap();
                if let Some(t) = s.by_id(&thread_id) {
                    t.running = true;
                }
            }
            let _ = tx.send(());
            match client.cycle_submit(&core_id, &input, None).await {
                Ok(cycle_id) => {
                    let mut s = state.lock().unwrap();
                    if let Some(t) = s.by_id(&thread_id) {
                        t.latest_cycle_id = Some(cycle_id);
                    }
                    Ok(())
                }
                Err(e) => {
                    {
                        let mut s = state.lock().unwrap();
                        if let Some(t) = s.by_id(&thread_id) {
                            t.running = false;
                        }
                    }
                    let _ = tx.send(());
                    Err(anyhow!(e.to_string()))
                }
            }
        })
    }

    fn abort(&self) {
        let client = self.client.clone();
        let cycle_id = {
            let s = self.state.lock().unwrap();
            s.active().latest_cycle_id.clone()
        };
        if let Some(cid) = cycle_id {
            tokio::spawn(async move {
                let _ = client.cycle_abort(&cid).await;
            });
        }
        self.ping();
    }

    fn new_session(&self) {
        let client = self.client.clone();
        let state = self.state.clone();
        let tx = self.tx.clone();
        let thread_id = {
            let mut s = self.state.lock().unwrap();
            let t = s.active_mut_reset();
            t.id.clone()
        };
        tokio::spawn(async move {
            if let Ok(core_id) = client.thread_create(Some("main"), Some("app")).await {
                let baseline = client
                    .thread_subscribe(&core_id, None)
                    .await
                    .ok()
                    .and_then(|v| v.get("baselineSeq").and_then(Value::as_u64))
                    .unwrap_or(0);
                let mut s = state.lock().unwrap();
                if let Some(t) = s.by_id(&thread_id) {
                    t.core_id = core_id;
                    t.seq_tracker = SeqTracker::new(baseline);
                }
            }
            let _ = tx.send(());
        });
        self.ping();
    }

    fn fork(&self, name: Option<String>) -> String {
        let (new_id, src_core, messages, chat_events) = {
            let mut s = self.state.lock().unwrap();
            let id = format!("t{}", s.next_thread);
            s.next_thread += 1;
            let (src_core, parent, messages, chat_events) = {
                let a = s.active();
                (
                    a.core_id.clone(),
                    a.id.clone(),
                    a.messages.clone(),
                    a.chat_events.clone(),
                )
            };
            let mut child = Thread::new(
                &id,
                &name.clone().unwrap_or_else(|| format!("fork {id}")),
                String::new(),
            );
            child.parent_id = Some(parent);
            child.messages = messages.clone();
            child.events = chat_events.clone();
            child.chat_events = chat_events.clone();
            s.threads.push(child);
            s.active_id = id.clone();
            (id, src_core, messages, chat_events)
        };
        let _ = (messages, chat_events);
        let client = self.client.clone();
        let state = self.state.clone();
        let tx = self.tx.clone();
        let thread_id = new_id.clone();
        tokio::spawn(async move {
            if !src_core.is_empty() {
                if let Ok(core_id) = client.thread_fork(&src_core, None).await {
                    let baseline = client
                        .thread_subscribe(&core_id, None)
                        .await
                        .ok()
                        .and_then(|v| v.get("baselineSeq").and_then(Value::as_u64))
                        .unwrap_or(0);
                    let mut s = state.lock().unwrap();
                    if let Some(t) = s.by_id(&thread_id) {
                        t.core_id = core_id;
                        t.seq_tracker = SeqTracker::new(baseline);
                    }
                }
            }
            let _ = tx.send(());
        });
        self.ping();
        new_id
    }

    fn set_active_thread(&self, id: String) {
        {
            let mut s = self.state.lock().unwrap();
            if s.threads.iter().any(|t| t.id == id) {
                s.active_id = id;
            }
        }
        self.ping();
    }

    fn list_main_chats(&self) -> BoxFuture<'static, anyhow::Result<Vec<MainChatSummary>>> {
        let client = self.client.clone();
        Box::pin(async move {
            let listed = client
                .thread_list()
                .await
                .map_err(|e| anyhow!(e.to_string()))?;
            let rows = listed
                .get("threads")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|t| {
                            let id = t.get("threadId").and_then(Value::as_str)?.to_string();
                            let name = t
                                .get("name")
                                .and_then(Value::as_str)
                                .filter(|s| !s.is_empty())
                                .unwrap_or(&id)
                                .to_string();
                            Some(MainChatSummary {
                                session_id: id,
                                name,
                                turns: t.get("cycleSeq").and_then(Value::as_u64).unwrap_or(0)
                                    as usize,
                                thread_count: 1,
                                updated_at: String::new(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok(rows)
        })
    }

    fn resume_chat(&self, main_session_id: String) -> BoxFuture<'static, anyhow::Result<()>> {
        let client = self.client.clone();
        let state = self.state.clone();
        let tx = self.tx.clone();
        Box::pin(async move {
            client
                .thread_resume(&main_session_id)
                .await
                .map_err(|e| anyhow!(e.to_string()))?;
            let sub = client
                .thread_subscribe(&main_session_id, None)
                .await
                .map_err(|e| anyhow!(e.to_string()))?;
            let baseline = sub.get("baselineSeq").and_then(Value::as_u64).unwrap_or(0);
            let mut s = state.lock().unwrap();
            if s.threads.iter().any(|t| t.running) {
                return Err(anyhow!("cannot resume while a thread is running"));
            }
            let base = s.seq;
            let mut seq = base;
            let synth = sub
                .get("snapshot")
                .map(|snap| synth_from_snapshot(snap, &mut seq))
                .unwrap_or_default();
            s.seq = seq;
            let id = s.active_id.clone();
            if let Some(t) = s.by_id(&id) {
                t.core_id = main_session_id.clone();
                t.events.clear();
                t.chat_events.clear();
                t.messages.clear();
                t.seq_tracker = SeqTracker::new(baseline);
                for env in synth {
                    if let TuiEvent::User { body } | TuiEvent::Assistant { body } = &env.event {
                        let role = if matches!(env.event, TuiEvent::User { .. }) {
                            "user"
                        } else {
                            "assistant"
                        };
                        t.messages.push(ChatMessage {
                            role: role.into(),
                            content: body.clone(),
                        });
                    }
                    State::push_event(t, env);
                }
            }
            drop(s);
            let _ = tx.send(());
            Ok(())
        })
    }

    fn set_async_mode(&self, on: bool) -> bool {
        {
            let mut s = self.state.lock().unwrap();
            s.async_mode = on;
        }
        self.ping();
        on
    }

    fn inspect_context(&self) -> BoxFuture<'static, anyhow::Result<Vec<ContextItem>>> {
        let client = self.client.clone();
        let cycle_id = { self.state.lock().unwrap().active().latest_cycle_id.clone() };
        Box::pin(async move {
            let Some(cid) = cycle_id else {
                return Ok(Vec::new());
            };
            let payload = client
                .context_inspect(&cid)
                .await
                .map_err(|e| anyhow!(e.to_string()))?;
            let items = payload
                .get("chunks")
                .and_then(Value::as_array)
                .map(|chunks| {
                    chunks
                        .iter()
                        .map(|c| {
                            let text = c
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            ContextItem {
                                ref_: c
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string(),
                                kind: c
                                    .get("kind")
                                    .and_then(Value::as_str)
                                    .unwrap_or("chunk")
                                    .to_string(),
                                bytes: text.len(),
                                content: text,
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok(items)
        })
    }

    fn shutdown(&self) -> BoxFuture<'static, anyhow::Result<()>> {
        self.closed.store(true, Ordering::Relaxed);
        Box::pin(async move { Ok(()) })
    }

    // --- steering & fleet ops ---------------------------------------------------

    fn answer_question(&self, cycle_id: String, question_id: String, body: String) {
        let client = self.client.clone();
        tokio::spawn(async move {
            let _ = client.question_answer(&cycle_id, &question_id, &body).await;
        });
        self.ping();
    }

    fn cancel_task(&self, cycle_id: String, task_id: String) {
        let client = self.client.clone();
        tokio::spawn(async move {
            let _ = client.task_cancel(&cycle_id, &task_id).await;
        });
        self.ping();
    }

    fn workers(&self) -> Vec<WorkerInfo> {
        self.state.lock().unwrap().workers.clone()
    }

    fn worker_op(&self, op: WorkerOp) -> BoxFuture<'static, anyhow::Result<()>> {
        let client = self.client.clone();
        let state = self.state.clone();
        let tx = self.tx.clone();
        Box::pin(async move {
            let result = match op {
                WorkerOp::Add {
                    address,
                    handle,
                    label,
                    harness,
                } => {
                    client
                        .worker_add(
                            address.as_deref(),
                            handle.as_deref(),
                            label.as_deref(),
                            harness.as_deref(),
                        )
                        .await
                }
                WorkerOp::Select { id } => client.worker_select(&id).await,
                WorkerOp::Update { id, patch } => {
                    client.worker_update(&id, Value::Object(patch)).await
                }
                WorkerOp::Remove { id } => client.worker_remove(&id).await,
            };
            match result {
                Ok(_) => {
                    // Re-pull the authoritative list (add/update return one row).
                    if let Ok(list) = client.worker_list().await {
                        state.lock().unwrap().workers = workers_from_payload(&list);
                    }
                    let _ = tx.send(());
                    Ok(())
                }
                Err(e) => Err(anyhow!(e.to_string())),
            }
        })
    }

    fn stream_state(&self) -> Option<StreamState> {
        let s = self.state.lock().unwrap();
        if self.closed.load(Ordering::Relaxed) {
            return Some(StreamState::Stalled);
        }
        if s.resyncing {
            return Some(StreamState::Resyncing);
        }
        if s.active().running && now_millis() - s.last_event_at > s.stall_ms {
            return Some(StreamState::Stalled);
        }
        Some(StreamState::Live)
    }
}

impl State {
    /// Reset the active thread's local state (keeps its id, clears its core binding).
    fn active_mut_reset(&mut self) -> &mut Thread {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::agents::{derive_agent_lanes, TaskStatus};

    fn ev(cycle: &str, body: Value) -> TuiEvent {
        map_core_event(&body, cycle)
    }

    #[test]
    fn task_complete_flat_wire_maps_to_nested_digest() {
        // §3.3(1): the wire body is flat; the mapper rebuilds the nested TaskDigest.
        let e = ev(
            "cyc:app:th_x:1",
            json!({"kind":"task_complete","taskId":"t1","status":"done","digest":"ok","usage":{"inputTokens":9,"outputTokens":2},"depth":1}),
        );
        match e {
            TuiEvent::TaskComplete { digest } => {
                assert_eq!(digest.task_id, "cyc:app:th_x:1/t:t1");
                assert_eq!(digest.status, "done");
                assert_eq!(digest.digest, "ok");
                assert_eq!(digest.usage.unwrap().input_tokens, 9);
            }
            other => panic!("expected task_complete, got {other:?}"),
        }
    }

    #[test]
    fn cycle_id_folds_into_the_lane_key() {
        // §3.3(2): two cycles delegating the bare `t1` never collide into one lane.
        let events: Vec<EventEnvelope> = [
            ev(
                "cyc:app:th:1",
                json!({"kind":"task_start","taskId":"t1","instruction":"a","depth":1}),
            ),
            ev(
                "cyc:app:th:2",
                json!({"kind":"task_start","taskId":"t1","instruction":"b","depth":1}),
            ),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, event)| EventEnvelope {
            seq: i as u64,
            at: i as i64,
            event,
        })
        .collect();
        let lanes = derive_agent_lanes(&events, "CORE", &[]);
        let workers: Vec<_> = lanes
            .iter()
            .filter(|l| l.key.starts_with("worker:"))
            .collect();
        assert_eq!(workers.len(), 2, "two distinct cycle-scoped lanes expected");
    }

    #[test]
    fn cancelled_status_is_distinct_from_failed() {
        // §3.3(3): cancelled ≠ failed.
        let events = vec![EventEnvelope {
            seq: 1,
            at: 1,
            event: ev(
                "cyc:app:th:1",
                json!({"kind":"task_complete","taskId":"t1","status":"cancelled","digest":""}),
            ),
        }];
        let lanes = derive_agent_lanes(&events, "CORE", &[]);
        let lane = lanes.iter().find(|l| l.key.starts_with("worker:")).unwrap();
        assert_eq!(lane.tasks[0].status, TaskStatus::Cancelled);
    }

    #[test]
    fn task_complete_without_task_start_still_lands_a_lane() {
        // §3.3(4): a completion whose task_start was evicted is not dropped.
        let events = vec![EventEnvelope {
            seq: 1,
            at: 1,
            event: ev(
                "cyc:app:th:1",
                json!({"kind":"task_complete","taskId":"orphan","status":"done","digest":"d"}),
            ),
        }];
        let lanes = derive_agent_lanes(&events, "CORE", &[]);
        let lane = lanes.iter().find(|l| l.key.starts_with("worker:"));
        assert!(lane.is_some(), "orphan completion must still create a lane");
        assert_eq!(lane.unwrap().tasks[0].status, TaskStatus::Done);
    }

    #[test]
    fn snapshot_rebuild_synthesizes_events() {
        let snapshot = json!({
            "at": 1000,
            "chat": [{"seq":1,"role":"user","body":"hi"},{"seq":2,"role":"assistant","body":"yo"}],
            "tasks": [{"taskId":"t1","cycleId":"cyc:app:th:1","status":"done","instruction":"go","digest":"done"}],
        });
        let mut seq = 0;
        let synth = synth_from_snapshot(&snapshot, &mut seq);
        let kinds: Vec<&str> = synth.iter().map(|e| e.event.kind()).collect();
        assert_eq!(
            kinds,
            vec!["user", "assistant", "task_start", "task_complete"]
        );
        assert_eq!(seq, 4);
    }

    #[test]
    fn workers_payload_parses_rows() {
        let payload = json!({
            "workers": [
                {"id":"w_1","address":"@dev","handle":"@dev","harness":"claude","selected":true},
                {"id":"w_2","address":"addr2"}
            ],
            "selectedId": "w_1"
        });
        let rows = workers_from_payload(&payload);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].selected);
        assert_eq!(rows[0].harness.as_deref(), Some("claude"));
        assert!(!rows[1].selected);
    }
}
