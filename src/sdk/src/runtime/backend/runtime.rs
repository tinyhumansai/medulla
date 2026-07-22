//! The [`Runtime`] trait surface over a live [`MedullaClient`]: connecting and
//! eager session creation, plus every trait method that issues HTTP requests,
//! renders snapshots, and drives thread lifecycle (submit, abort, new session,
//! fork, resume, shutdown).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use futures::future::BoxFuture;
use tokio::sync::broadcast;

use crate::client::{
    FeedbackComment, FeedbackDetail, FeedbackItem, FeedbackPage, FeedbackQuery, FeedbackSubmission,
    FeedbackType, MedullaClient, Role,
};
use crate::runtime::{
    AgentDescriptor, AgentPresence, ContextItem, PeerSession, Runtime, RuntimeSnapshot,
    TinyplaceIdentity,
};
use crate::ui::chat_store::{now_millis, ChatMessage, MainChatSummary};
use crate::ui::events::{EventEnvelope, TuiEvent};

use super::fold::summary_from_session;
use super::stream::start_stream_on;
use super::types::{BackendRuntime, State, Thread};

impl BackendRuntime {
    /// Connect and eagerly create the initial backend session, then attach its
    /// stream. Eager creation is chosen over lazy-on-first-submit because it
    /// keeps every thread's stream-task lifecycle uniform (a thread always has a
    /// session to stream).
    pub async fn connect(client: MedullaClient) -> anyhow::Result<Self> {
        Self::connect_with_hub(client, Arc::new(Mutex::new(None))).await
    }

    /// Like [`connect`](Self::connect) but attaches a shared hub slot. The caller
    /// fills the slot once the orchestrator hub connects, so `workers()` /
    /// `worker_op()` manage the hub's tiny.place peers instead of being no-ops.
    ///
    /// # Errors
    ///
    /// Returns an error if the eager initial backend-session creation fails
    /// (network failure, auth rejection, or a non-2xx response from the backend);
    /// the runtime is not constructed in that case.
    pub async fn connect_with_hub(
        client: MedullaClient,
        hub: Arc<Mutex<Option<crate::hub::HubHandle>>>,
    ) -> anyhow::Result<Self> {
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
        let rt = BackendRuntime {
            client,
            state,
            tx,
            hub,
        };
        start_stream_on(&rt.client, &rt.state, &rt.tx, "t1", None);
        Ok(rt)
    }

    /// Notify subscribers to re-pull a snapshot.
    fn ping(&self) {
        let _ = self.tx.send(());
    }
}

impl Runtime for BackendRuntime {
    fn describe(&self) -> String {
        format!("backend {}", self.client.base_url())
    }

    /// The hub's live tiny.place worker roster (empty until a hub is attached).
    fn workers(&self) -> Vec<crate::runtime::WorkerInfo> {
        let handle = self.hub.lock().unwrap().clone();
        match handle {
            Some(h) => h.list().into_iter().map(hub_worker_to_info).collect(),
            None => Vec::new(),
        }
    }

    /// Apply a Workers-tab mutation to the hub (add/remove/relabel/select),
    /// re-registering with the backend on change.
    fn worker_op(&self, op: crate::runtime::WorkerOp) -> BoxFuture<'static, anyhow::Result<()>> {
        let handle = self.hub.lock().unwrap().clone();
        Box::pin(async move {
            match handle {
                Some(h) => apply_worker_op(&h, op).await,
                // Reading an empty roster is honest; silently succeeding at a
                // *mutation* that did not happen is not. Without a hub there is
                // nothing to add a worker to, and reporting "updated" leaves the
                // operator watching for a peer that was never registered.
                None => Err(anyhow!(
                    "no orchestrator hub is attached — sign in and restart, or set MEDULLA_HUB_WORKERS"
                )),
            }
        })
    }

    fn team_usage(&self) -> BoxFuture<'static, anyhow::Result<Option<serde_json::Value>>> {
        let client = self.client.clone();
        Box::pin(async move { Ok(Some(client.team_usage().await?)) })
    }

    // --- feedback board ---------------------------------------------------
    // Straight pass-through to the client; the board is entirely server-state,
    // so nothing is cached in `State`.

    fn list_feedback(
        &self,
        query: FeedbackQuery,
    ) -> BoxFuture<'static, anyhow::Result<Option<FeedbackPage>>> {
        let client = self.client.clone();
        Box::pin(async move { Ok(Some(client.list_feedback(&query).await?)) })
    }

    fn feedback_detail(&self, id: String) -> BoxFuture<'static, anyhow::Result<FeedbackDetail>> {
        let client = self.client.clone();
        Box::pin(async move { Ok(client.get_feedback(&id).await?) })
    }

    fn vote_feedback(
        &self,
        id: String,
        value: i8,
    ) -> BoxFuture<'static, anyhow::Result<FeedbackItem>> {
        let client = self.client.clone();
        Box::pin(async move { Ok(client.vote_feedback(&id, value).await?) })
    }

    fn comment_feedback(
        &self,
        id: String,
        body: String,
    ) -> BoxFuture<'static, anyhow::Result<FeedbackComment>> {
        let client = self.client.clone();
        Box::pin(async move { Ok(client.comment_feedback(&id, &body).await?) })
    }

    fn submit_feedback(
        &self,
        kind: FeedbackType,
        title: String,
        body: String,
    ) -> BoxFuture<'static, anyhow::Result<FeedbackSubmission>> {
        let client = self.client.clone();
        Box::pin(async move { Ok(client.submit_feedback(kind, &title, &body).await?) })
    }

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
            // The hub's own identity, so the operator can read off the address a
            // worker must trust (owner / allowlist) before it accepts a task.
            tinyplace: self
                .hub
                .lock()
                .unwrap()
                .as_ref()
                .map(|h| TinyplaceIdentity {
                    agent_id: h.address().to_string(),
                    public_key: h.public_key().to_string(),
                    handle: None,
                }),
            async_mode: s.async_mode,
            threads,
            active_thread_id: s.active_id.clone(),
            harness: None,
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

/// Map a hub roster entry to the UI's [`WorkerInfo`](crate::runtime::WorkerInfo)
/// row. An `@handle` address is surfaced as the `handle` field too.
///
/// `pub(super)` so the module's sibling test module can pin the mapping without
/// standing up a live hub handle.
pub(super) fn hub_worker_to_info(w: crate::hub::HubWorker) -> crate::runtime::WorkerInfo {
    crate::runtime::WorkerInfo {
        handle: w.address.starts_with('@').then(|| w.address.clone()),
        id: w.id,
        address: w.address,
        label: w.label,
        harness: Some(w.harness),
        peer_id: None,
        selected: w.selected,
    }
}

/// Translate a [`WorkerOp`](crate::runtime::WorkerOp) into a hub-handle mutation.
async fn apply_worker_op(
    handle: &crate::hub::HubHandle,
    op: crate::runtime::WorkerOp,
) -> anyhow::Result<()> {
    use crate::runtime::WorkerOp;
    match op {
        WorkerOp::Add {
            address,
            handle: h,
            label,
            harness,
        } => {
            let addr = address
                .or(h)
                .ok_or_else(|| anyhow!("a worker needs an address or @handle"))?;
            handle
                .add(crate::hub::HubWorker {
                    id: addr.clone(),
                    address: addr,
                    harness: harness.unwrap_or_else(|| "claude".to_string()),
                    label,
                    selected: false,
                })
                .await
        }
        WorkerOp::Remove { id } => handle.remove(&id).await,
        WorkerOp::Update { id, patch } => {
            let label = patch
                .get("label")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .filter(|s| !s.is_empty());
            handle.set_label(&id, label).await
        }
        WorkerOp::Select { id } => {
            handle.select(&id);
            Ok(())
        }
    }
}
