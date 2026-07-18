//! A [`Runtime`] backed by the live Medulla backend HTTP + SSE API.
//!
//! Threads map to backend sessions. Each thread runs its own SSE task that
//! folds the backend's `EventEnvelope`s into the thread's local event log, from
//! which snapshots are rendered. State lives behind an `Arc<Mutex<...>>` and a
//! tokio broadcast channel notifies the UI to re-pull a snapshot after every
//! fold, exactly like [`MockRuntime`](crate::runtime::mock::MockRuntime).
//!
//! Divergences from the mock / TS runtime, all because the backend does not (yet)
//! expose the surface:
//! - `fork` has no backend equivalent — the backend has no fork endpoint. We
//!   open a *fresh* session and copy the parent thread's transcript locally, so
//!   the fork diverges from its parent server-side from the first turn.
//! - `set_async_mode` is a purely local flag; the `/medulla/v1` message endpoint
//!   is always called async (`sync=0`) regardless. It changes nothing
//!   server-side and is kept only so the UI toggle has somewhere to land.
//! - `inspect_context` returns an empty list — the backend does not expose the
//!   context store over HTTP.
//! - Roster / presence / peer-session data is empty — that fleet data arrives
//!   over Socket.IO, which this runtime does not open.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use futures::future::BoxFuture;
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::client::{
    EventEnvelope as ClientEnvelope, EventKind, MedullaClient, Role, SessionSummary,
};

use crate::runtime::{
    AgentDescriptor, AgentPresence, ContextItem, CycleResultSummary, PeerSession, Runtime,
    RuntimeSnapshot, ThreadSummary, TinyplaceIdentity,
};
use crate::ui::chat_store::{iso8601_utc, now_millis, ChatMessage, MainChatSummary};
use crate::ui::events::{EventEnvelope, TuiEvent};

const EVENT_CAP: usize = 5000;
const CHAT_CAP: usize = 2000;

/// One local thread over a backend session.
struct Thread {
    id: String,
    parent_id: Option<String>,
    name: String,
    /// Backend session id; empty while a session is being created.
    session_id: String,
    messages: Vec<ChatMessage>,
    events: Vec<EventEnvelope>,
    chat_events: Vec<EventEnvelope>,
    running: bool,
    last_result: Option<CycleResultSummary>,
    /// A user message appended optimistically on submit, awaiting its echo from
    /// the stream so the folded echo can be de-duplicated.
    pending_user_echo: Option<String>,
    stream_task: Option<JoinHandle<()>>,
}

impl Thread {
    fn new(id: &str, name: &str, session_id: String) -> Self {
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

    fn reset(&mut self) {
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

struct State {
    threads: Vec<Thread>,
    active_id: String,
    /// Monotonic local sequence counter assigned to every folded event.
    seq: u64,
    next_thread: usize,
    async_mode: bool,
}

impl State {
    fn active(&self) -> &Thread {
        self.threads
            .iter()
            .find(|t| t.id == self.active_id)
            .expect("active thread")
    }

    fn active_mut(&mut self) -> &mut Thread {
        let id = self.active_id.clone();
        self.threads
            .iter_mut()
            .find(|t| t.id == id)
            .expect("active thread")
    }

    fn by_id(&mut self, id: &str) -> Option<&mut Thread> {
        self.threads.iter_mut().find(|t| t.id == id)
    }

    fn by_session(&mut self, session_id: &str) -> Option<&mut Thread> {
        self.threads.iter_mut().find(|t| t.session_id == session_id)
    }

    /// Push a fully-formed local event into a thread, applying both caps and the
    /// chat-events subset filter.
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

    /// Optimistically append the just-submitted user turn to the active thread.
    fn push_local_user(&mut self, thread_id: &str, body: &str, at: i64) {
        self.seq += 1;
        let seq = self.seq;
        if let Some(t) = self.by_id(thread_id) {
            t.running = true;
            t.pending_user_echo = Some(body.to_string());
            t.messages.push(ChatMessage {
                role: "user".into(),
                content: body.to_string(),
            });
            Self::push_event(
                t,
                EventEnvelope {
                    seq,
                    at,
                    event: TuiEvent::User {
                        body: body.to_string(),
                    },
                },
            );
        }
    }

    /// Append a locally-sourced error (e.g. a failed send) and clear running.
    fn push_local_error(&mut self, thread_id: &str, source: &str, message: &str) {
        self.seq += 1;
        let seq = self.seq;
        if let Some(t) = self.by_id(thread_id) {
            t.running = false;
            Self::push_event(
                t,
                EventEnvelope {
                    seq,
                    at: now_millis(),
                    event: TuiEvent::Error {
                        source: source.into(),
                        message: message.into(),
                    },
                },
            );
        }
    }

    /// Fold one backend event into the thread bound to `session_id`. Returns the
    /// local seq assigned, or `None` when the event was de-duplicated or the
    /// thread is gone.
    fn fold(&mut self, session_id: &str, env: &ClientEnvelope) -> Option<u64> {
        let kind = env.kind();
        // De-duplicate the echo of an optimistically-appended user turn.
        if let EventKind::User { body } = &kind {
            if let Some(t) = self.by_session(session_id) {
                if t.pending_user_echo.as_deref() == Some(body.as_str()) {
                    t.pending_user_echo = None;
                    return None;
                }
            }
        }

        self.seq += 1;
        let seq = self.seq;
        let at = env.at as i64;
        let event = map_event(kind);

        let t = self.by_session(session_id)?;
        match &event {
            TuiEvent::User { body } => t.messages.push(ChatMessage {
                role: "user".into(),
                content: body.clone(),
            }),
            TuiEvent::Assistant { body } => t.messages.push(ChatMessage {
                role: "assistant".into(),
                content: body.clone(),
            }),
            TuiEvent::CycleEnd { pass_count, .. } => {
                t.running = false;
                t.last_result = Some(CycleResultSummary {
                    pass_count: *pass_count,
                    task_ledger: HashMap::new(),
                });
            }
            TuiEvent::CycleStart { .. } => t.running = true,
            _ => {}
        }
        Self::push_event(t, EventEnvelope { seq, at, event });
        Some(seq)
    }

    fn thread_summaries(&self) -> Vec<ThreadSummary> {
        self.threads
            .iter()
            .map(|t| {
                let mut attention = 0usize;
                for env in &t.events {
                    if matches!(env.event, TuiEvent::Error { .. }) {
                        attention += 1;
                    }
                }
                ThreadSummary {
                    id: t.id.clone(),
                    parent_id: t.parent_id.clone(),
                    name: t.name.clone(),
                    running: t.running,
                    turns: t.messages.len().div_ceil(2),
                    running_tasks: 0,
                    attention,
                }
            })
            .collect()
    }
}

/// Map a client [`EventKind`] onto the TUI's [`TuiEvent`] vocabulary.
fn map_event(kind: EventKind) -> TuiEvent {
    match kind {
        EventKind::User { body } => TuiEvent::User { body },
        EventKind::Assistant { body } => TuiEvent::Assistant { body },
        EventKind::CycleStart { cycle_id } => TuiEvent::CycleStart {
            cycle_id: cycle_id.unwrap_or_default(),
        },
        EventKind::CycleEnd {
            cycle_id,
            pass_count,
            duration_ms,
            ..
        } => TuiEvent::CycleEnd {
            cycle_id: cycle_id.unwrap_or_default(),
            pass_count: pass_count.unwrap_or(0) as i64,
            duration_ms: duration_ms.unwrap_or(0) as i64,
        },
        EventKind::Error { source, message } => TuiEvent::Error { source, message },
        EventKind::AssistantDelta { delta } => TuiEvent::AssistantDelta { delta },
        EventKind::ReasoningDelta { delta } => TuiEvent::ReasoningDelta { delta },
        EventKind::ToolCallDelta { value } => TuiEvent::ToolCallDelta {
            index: value.get("index").and_then(Value::as_i64).unwrap_or(0),
            args_delta: value
                .get("argsDelta")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        },
        EventKind::Unknown(v) => TuiEvent::Unknown {
            kind: v
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            data: v.as_object().cloned().unwrap_or_default(),
        },
    }
}

/// Map a backend session summary onto a resume-picker row. `turns` approximates
/// `lastSeq / 2` (one user + one assistant per turn).
fn summary_from_session(s: &SessionSummary) -> MainChatSummary {
    let name = s
        .title
        .clone()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| s.session_id.clone());
    let turns = s.last_seq.unwrap_or(0).max(0) as usize / 2;
    MainChatSummary {
        session_id: s.session_id.clone(),
        name,
        turns,
        thread_count: 1,
        updated_at: iso8601_utc(s.last_active_at.unwrap_or(0)),
    }
}

/// Spawn the per-thread SSE loop: fold each envelope and ping after every fold.
fn spawn_stream(
    client: MedullaClient,
    state: Arc<Mutex<State>>,
    tx: broadcast::Sender<()>,
    session_id: String,
    cursor: Option<u64>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let stream = client.stream_events(&session_id, cursor);
        futures::pin_mut!(stream);
        while let Some(item) = stream.next().await {
            if let Ok(env) = item {
                {
                    let mut s = state.lock().unwrap();
                    s.fold(&session_id, &env);
                }
                // Ping after every fold so the UI re-pulls a snapshot.
                let _ = tx.send(());
            }
            // Transient stream errors are swallowed; the client stream
            // reconnects internally from its cursor.
        }
    })
}

/// Attach a fresh SSE task to the thread `thread_id`, replacing any prior one.
fn start_stream_on(
    client: &MedullaClient,
    state: &Arc<Mutex<State>>,
    tx: &broadcast::Sender<()>,
    thread_id: &str,
    cursor: Option<u64>,
) {
    let mut s = state.lock().unwrap();
    let Some(t) = s.by_id(thread_id) else {
        return;
    };
    if t.session_id.is_empty() {
        return;
    }
    if let Some(h) = t.stream_task.take() {
        h.abort();
    }
    let session_id = t.session_id.clone();
    let handle = spawn_stream(
        client.clone(),
        state.clone(),
        tx.clone(),
        session_id,
        cursor,
    );
    if let Some(t) = s.by_id(thread_id) {
        t.stream_task = Some(handle);
    }
}

/// A [`Runtime`] over a live [`MedullaClient`].
pub struct BackendRuntime {
    client: MedullaClient,
    state: Arc<Mutex<State>>,
    tx: broadcast::Sender<()>,
}

impl BackendRuntime {
    /// Connect and eagerly create the initial backend session, then attach its
    /// stream. Eager creation is chosen over lazy-on-first-submit because it
    /// keeps every thread's stream-task lifecycle uniform (a thread always has a
    /// session to stream).
    pub async fn connect(client: MedullaClient) -> anyhow::Result<Self> {
        let created = client
            .create_session(None)
            .await
            .map_err(|e| anyhow!(e.to_string()))?;
        let (tx, _rx) = broadcast::channel(256);
        let state = Arc::new(Mutex::new(State {
            threads: vec![Thread::new("t1", "main", created.session_id)],
            active_id: "t1".into(),
            seq: 0,
            next_thread: 2,
            async_mode: false,
        }));
        let rt = BackendRuntime { client, state, tx };
        start_stream_on(&rt.client, &rt.state, &rt.tx, "t1", None);
        Ok(rt)
    }

    fn ping(&self) {
        let _ = self.tx.send(());
    }
}

impl Runtime for BackendRuntime {
    fn snapshot(&self) -> RuntimeSnapshot {
        let s = self.state.lock().unwrap();
        let threads = s.thread_summaries();
        let active = s.active();
        RuntimeSnapshot {
            session_id: active.session_id.clone(),
            running: active.running,
            events: active.events.clone(),
            chat_events: active.chat_events.clone(),
            messages: active.messages.clone(),
            last_result: active.last_result.clone(),
            tracing: false,
            roster: Vec::<AgentDescriptor>::new(),
            presence: HashMap::<String, AgentPresence>::new(),
            sessions: HashMap::<String, Vec<PeerSession>>::new(),
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
            let (session_id, thread_id) = {
                let s = state.lock().unwrap();
                let t = s.active();
                if t.session_id.is_empty() {
                    return Err(anyhow!("session is still being created; try again"));
                }
                if t.running {
                    return Err(anyhow!("a cycle is already running"));
                }
                (t.session_id.clone(), t.id.clone())
            };
            {
                let mut s = state.lock().unwrap();
                s.push_local_user(&thread_id, &input, now_millis());
            }
            let _ = tx.send(());
            match client.send_message(&session_id, &input, false).await {
                Ok(_) => Ok(()),
                Err(e) => {
                    {
                        let mut s = state.lock().unwrap();
                        s.push_local_error(&thread_id, "cycle", &e.to_string());
                    }
                    let _ = tx.send(());
                    Err(anyhow!(e.to_string()))
                }
            }
        })
    }

    fn abort(&self) {
        let client = self.client.clone();
        let session_id = {
            let s = self.state.lock().unwrap();
            s.active().session_id.clone()
        };
        if !session_id.is_empty() {
            tokio::spawn(async move {
                let _ = client.abort(&session_id).await;
            });
        }
        self.ping();
    }

    fn new_session(&self) {
        let thread_id = {
            let mut s = self.state.lock().unwrap();
            let t = s.active_mut();
            t.reset();
            t.session_id.clear();
            t.id.clone()
        };
        let client = self.client.clone();
        let state = self.state.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            if let Ok(created) = client.create_session(None).await {
                {
                    let mut s = state.lock().unwrap();
                    if let Some(t) = s.by_id(&thread_id) {
                        t.session_id = created.session_id;
                    }
                }
                start_stream_on(&client, &state, &tx, &thread_id, None);
                let _ = tx.send(());
            }
        });
        self.ping();
    }

    fn fork(&self, name: Option<String>) -> String {
        let new_id = {
            let mut s = self.state.lock().unwrap();
            let id = format!("t{}", s.next_thread);
            s.next_thread += 1;
            let (parent_id, messages, chat_events) = {
                let active = s.active();
                (
                    active.id.clone(),
                    active.messages.clone(),
                    active.chat_events.clone(),
                )
            };
            let mut child = Thread::new(
                &id,
                &name.unwrap_or_else(|| format!("fork {id}")),
                String::new(),
            );
            child.parent_id = Some(parent_id);
            // Copy the parent transcript locally; the backend has no fork, so the
            // fresh session below starts empty and diverges from here on.
            child.messages = messages;
            child.events = chat_events.clone();
            child.chat_events = chat_events;
            s.threads.push(child);
            s.active_id = id.clone();
            id
        };
        let client = self.client.clone();
        let state = self.state.clone();
        let tx = self.tx.clone();
        let thread_id = new_id.clone();
        tokio::spawn(async move {
            if let Ok(created) = client.create_session(None).await {
                {
                    let mut s = state.lock().unwrap();
                    if let Some(t) = s.by_id(&thread_id) {
                        t.session_id = created.session_id;
                    }
                }
                start_stream_on(&client, &state, &tx, &thread_id, None);
                let _ = tx.send(());
            }
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
            let sessions = client
                .list_sessions()
                .await
                .map_err(|e| anyhow!(e.to_string()))?;
            Ok(sessions.iter().map(summary_from_session).collect())
        })
    }

    fn resume_chat(&self, main_session_id: String) -> BoxFuture<'static, anyhow::Result<()>> {
        let client = self.client.clone();
        let state = self.state.clone();
        let tx = self.tx.clone();
        Box::pin(async move {
            let messages = client
                .list_messages(&main_session_id, None)
                .await
                .map_err(|e| anyhow!(e.to_string()))?;
            let cursor = client
                .get_session(&main_session_id)
                .await
                .ok()
                .and_then(|d| d.event_seq)
                .filter(|e| *e >= 0)
                .map(|e| e as u64);

            let thread_id = {
                let mut s = state.lock().unwrap();
                if s.threads.iter().any(|t| t.running) {
                    return Err(anyhow!("cannot resume while a thread is running"));
                }
                let t = s.active_mut();
                t.reset();
                t.session_id = main_session_id.clone();
                let id = t.id.clone();
                for m in &messages {
                    let role = match m.role {
                        Role::User => "user",
                        Role::Assistant => "assistant",
                        Role::Other => continue,
                    };
                    s.seq += 1;
                    let seq = s.seq;
                    let at = m.ts.unwrap_or(0);
                    let event = if role == "user" {
                        TuiEvent::User {
                            body: m.body.clone(),
                        }
                    } else {
                        TuiEvent::Assistant {
                            body: m.body.clone(),
                        }
                    };
                    if let Some(t) = s.by_id(&id) {
                        t.messages.push(ChatMessage {
                            role: role.into(),
                            content: m.body.clone(),
                        });
                        State::push_event(t, EventEnvelope { seq, at, event });
                    }
                }
                id
            };
            start_stream_on(&client, &state, &tx, &thread_id, cursor);
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
        // The backend does not expose the context store over HTTP yet.
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn shutdown(&self) -> BoxFuture<'static, anyhow::Result<()>> {
        {
            let mut s = self.state.lock().unwrap();
            for t in &mut s.threads {
                if let Some(h) = t.stream_task.take() {
                    h.abort();
                }
            }
        }
        Box::pin(async move { Ok(()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn client_env(session: &str, seq: Option<u64>, event: Value) -> ClientEnvelope {
        let mut raw = json!({
            "at": 1234u64,
            "sessionId": session,
            "event": event,
        });
        if let Some(seq) = seq {
            raw["seq"] = json!(seq);
        }
        serde_json::from_value(raw).unwrap()
    }

    fn state_with_thread() -> State {
        State {
            threads: vec![Thread::new("t1", "main", "sess-1".into())],
            active_id: "t1".into(),
            seq: 0,
            next_thread: 2,
            async_mode: false,
        }
    }

    #[test]
    fn folds_kinds_into_tui_events() {
        let mut s = state_with_thread();
        s.fold(
            "sess-1",
            &client_env("sess-1", Some(1), json!({"kind":"user","body":"hi"})),
        );
        s.fold(
            "sess-1",
            &client_env("sess-1", Some(2), json!({"kind":"assistant","body":"yo"})),
        );
        s.fold(
            "sess-1",
            &client_env(
                "sess-1",
                None,
                json!({"kind":"assistant_delta","delta":"y"}),
            ),
        );
        s.fold(
            "sess-1",
            &client_env(
                "sess-1",
                None,
                json!({"kind":"reasoning_delta","delta":"r"}),
            ),
        );
        s.fold(
            "sess-1",
            &client_env(
                "sess-1",
                None,
                json!({"kind":"tool_call_delta","index":2,"argsDelta":"{"}),
            ),
        );
        s.fold(
            "sess-1",
            &client_env("sess-1", Some(3), json!({"kind":"weird","x":1})),
        );

        let t = &s.threads[0];
        let kinds: Vec<&str> = t.events.iter().map(|e| e.event.kind()).collect();
        assert_eq!(
            kinds,
            vec![
                "user",
                "assistant",
                "assistant_delta",
                "reasoning_delta",
                "tool_call_delta",
                "weird"
            ]
        );
        // Chat events are the user/assistant/error subset only.
        let chat: Vec<&str> = t.chat_events.iter().map(|e| e.event.kind()).collect();
        assert_eq!(chat, vec!["user", "assistant"]);
        // Messages track user + assistant turns.
        assert_eq!(t.messages.len(), 2);
        // The tool-call delta carried its fields through.
        match &t.events[4].event {
            TuiEvent::ToolCallDelta { index, args_delta } => {
                assert_eq!(*index, 2);
                assert_eq!(args_delta, "{");
            }
            other => panic!("expected tool_call_delta, got {other:?}"),
        }
        // Envelope `at` came from the client envelope.
        assert_eq!(t.events[0].at, 1234);
    }

    #[test]
    fn cycle_bracket_drives_running_and_last_result() {
        let mut s = state_with_thread();
        s.threads[0].running = true;
        s.fold(
            "sess-1",
            &client_env(
                "sess-1",
                Some(1),
                json!({"kind":"cycle_start","cycleId":"c1"}),
            ),
        );
        assert!(s.threads[0].running);
        s.fold(
            "sess-1",
            &client_env(
                "sess-1",
                Some(2),
                json!({"kind":"cycle_end","cycleId":"c1","passCount":3,"durationMs":42}),
            ),
        );
        assert!(!s.threads[0].running);
        let lr = s.threads[0].last_result.as_ref().unwrap();
        assert_eq!(lr.pass_count, 3);
    }

    #[test]
    fn optimistic_user_echo_is_deduped() {
        let mut s = state_with_thread();
        s.push_local_user("t1", "hello", 10);
        assert_eq!(s.threads[0].events.len(), 1);
        assert!(s.threads[0].running);
        // The stream echoes the same user turn — it must not duplicate.
        let folded = s.fold(
            "sess-1",
            &client_env("sess-1", Some(1), json!({"kind":"user","body":"hello"})),
        );
        assert!(folded.is_none());
        assert_eq!(s.threads[0].events.len(), 1);
        assert_eq!(s.threads[0].messages.len(), 1);
        // A different user turn is not swallowed.
        let folded = s.fold(
            "sess-1",
            &client_env("sess-1", Some(2), json!({"kind":"user","body":"world"})),
        );
        assert!(folded.is_some());
        assert_eq!(s.threads[0].messages.len(), 2);
    }

    #[test]
    fn event_logs_are_capped() {
        let mut s = state_with_thread();
        for i in 0..(EVENT_CAP + 200) {
            let body = format!("m{i}");
            // Alternate a chatty and a non-chatty event to exercise both caps.
            if i % 2 == 0 {
                s.fold(
                    "sess-1",
                    &client_env(
                        "sess-1",
                        Some(i as u64),
                        json!({"kind":"assistant","body":body}),
                    ),
                );
            } else {
                s.fold(
                    "sess-1",
                    &client_env(
                        "sess-1",
                        None,
                        json!({"kind":"assistant_delta","delta":body}),
                    ),
                );
            }
        }
        let t = &s.threads[0];
        assert_eq!(t.events.len(), EVENT_CAP);
        assert!(t.chat_events.len() <= CHAT_CAP);
    }

    #[test]
    fn session_summary_maps_to_main_chat() {
        let s: SessionSummary = serde_json::from_value(json!({
            "sessionId": "sess-9",
            "title": "Auth refactor",
            "lastActiveAt": 1_700_000_000_000i64,
            "status": "active",
            "lastSeq": 7,
        }))
        .unwrap();
        let row = summary_from_session(&s);
        assert_eq!(row.session_id, "sess-9");
        assert_eq!(row.name, "Auth refactor");
        assert_eq!(row.turns, 3); // 7 / 2
        assert_eq!(row.thread_count, 1);
        assert_eq!(row.updated_at, "2023-11-14T22:13:20.000Z");
    }

    #[test]
    fn session_summary_falls_back_to_id_for_name() {
        let s: SessionSummary = serde_json::from_value(json!({
            "sessionId": "sess-bare",
            "status": "idle",
        }))
        .unwrap();
        let row = summary_from_session(&s);
        assert_eq!(row.name, "sess-bare");
        assert_eq!(row.turns, 0);
    }
}
