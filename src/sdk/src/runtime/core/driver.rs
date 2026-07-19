//! The [`Runtime`] trait implementation for [`CoreRuntime`]: the synchronous
//! snapshot/subscribe surface plus the async cycle-submit, thread fork/resume,
//! context inspection, worker-registry, and persona-memory operations the UI drives.

use std::sync::atomic::Ordering;

use anyhow::anyhow;
use futures::future::BoxFuture;
use serde_json::Value;
use tokio::sync::broadcast;

use crate::memory::{MemoryHit, MemoryStatus};
use crate::runtime::core_client::SeqTracker;
use crate::runtime::{
    AgentDescriptor, ContextItem, Runtime, RuntimeSnapshot, StreamState, TinyplaceIdentity,
    WorkerInfo, WorkerOp,
};
use crate::ui::chat_store::{now_millis, ChatMessage, MainChatSummary};
use crate::ui::events::TuiEvent;

use super::events::synth_from_snapshot;
use super::types::{CoreRuntime, State, Thread};
use super::workers::workers_from_payload;

impl Runtime for CoreRuntime {
    fn describe(&self) -> String {
        format!("core {}", self.client.socket_path().display())
    }

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

    fn memory_status(&self) -> Option<MemoryStatus> {
        self.memory.as_ref().map(|m| m.status())
    }

    fn memory_search(&self, query: String, facet: Option<String>, k: usize) -> Vec<MemoryHit> {
        match &self.memory {
            Some(m) => m.search(&query, facet.as_deref(), k),
            None => Vec::new(),
        }
    }

    fn memory_directives(&self) -> Vec<String> {
        self.memory
            .as_ref()
            .map(|m| m.directives())
            .unwrap_or_default()
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
