//! The [`Runtime`] trait implementation for [`MockRuntime`]: snapshotting,
//! subscription, the scripted submit/abort/session lifecycle, and forking.
//! This is the behaviour half of the mock; the data model it drives lives in
//! [`super::types`].

use std::collections::HashMap;

use futures::future::BoxFuture;
use tokio::sync::broadcast;

use crate::runtime::{ContextItem, CycleResultSummary, Runtime, RuntimeSnapshot};
use crate::ui::chat_store::{ChatMessage, MainChatSummary};
use crate::ui::events::{TuiEvent, Usage};

use super::types::{gen_id, now_millis, MockRuntime, Thread};
use crate::runtime::event_log::ThreadEventLog;

impl Runtime for MockRuntime {
    fn describe(&self) -> String {
        "mock (scripted)".into()
    }

    fn snapshot(&self) -> RuntimeSnapshot {
        let s = self.state.lock().unwrap();
        let threads = Self::thread_summaries(&s);
        let active = s.active();
        RuntimeSnapshot {
            session_id: active.session_id.clone(),
            running: active.running,
            events: active.events.clone(),
            chat_events: active.chat_events.clone(),
            messages: active.messages.clone(),
            last_result: active.last_result.clone(),
            tracing: s.tracing,
            roster: s.roster.clone(),
            capacity: s.capacity.clone(),
            presence: s.presence.clone(),
            sessions: s.sessions.clone(),
            tinyplace: s.tinyplace.clone(),
            threads,
            active_thread_id: s.active_id.clone(),
            harness: s.harness.clone(),
            // The mock runtime never rebaselines its log; seqs only grow.
            replay_epoch: 0,
        }
    }

    /// The scripted worker registry (see
    /// [`MockRuntime::set_workers`](super::types::MockRuntime::set_workers)).
    fn workers(&self) -> Vec<crate::runtime::WorkerInfo> {
        self.state.lock().unwrap().workers.clone()
    }

    fn subscribe(&self) -> broadcast::Receiver<()> {
        self.tx.subscribe()
    }

    fn submit(&self, input: String) -> BoxFuture<'static, anyhow::Result<()>> {
        self.record("submit");
        let state = self.state.clone();
        let tx = self.tx.clone();
        Box::pin(async move {
            {
                let mut s = state.lock().unwrap();
                if s.active().running {
                    return Err(anyhow::anyhow!("a cycle is already running"));
                }
                s.cycle_seq += 1;
                let cid = format!("cyc-{}", s.cycle_seq);
                s.active_mut().running = true;
                s.emit(TuiEvent::CycleStart {
                    cycle_id: cid.clone(),
                });
                s.emit(TuiEvent::User {
                    body: input.clone(),
                });
                s.active_mut().messages.push(ChatMessage {
                    role: "user".into(),
                    content: input,
                });
                s.emit(TuiEvent::InferenceStart {
                    tier: "reasoning".into(),
                    op: "execute_step".into(),
                    model: Some("gpt-4o".into()),
                });
            }
            let _ = tx.send(());
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            {
                let mut s = state.lock().unwrap();
                s.emit(TuiEvent::InferenceEnd {
                    tier: "reasoning".into(),
                    op: "execute_step".into(),
                    model: Some("gpt-4o".into()),
                    duration_ms: 500,
                    usage: Some(Usage {
                        input_tokens: 800,
                        output_tokens: 120,
                        ..Default::default()
                    }),
                    content: Some("Here is my reasoning.".into()),
                    reasoning: None,
                    tool_calls: None,
                });
                let reply = "(mock) I processed your request.".to_string();
                s.emit(TuiEvent::Assistant {
                    body: reply.clone(),
                });
                s.active_mut().messages.push(ChatMessage {
                    role: "assistant".into(),
                    content: reply,
                });
                let cid = format!("cyc-{}", s.cycle_seq);
                s.emit(TuiEvent::CycleEnd {
                    cycle_id: cid,
                    pass_count: 1,
                    duration_ms: 500,
                });
                s.active_mut().running = false;
                s.active_mut().last_result = Some(CycleResultSummary {
                    pass_count: 1,
                    task_ledger: HashMap::new(),
                });
            }
            let _ = tx.send(());
            Ok(())
        })
    }

    fn abort(&self) {
        self.record("abort");
        {
            let mut s = self.state.lock().unwrap();
            if s.active().running {
                s.emit(TuiEvent::Error {
                    source: "operator".into(),
                    message: "Abort requested".into(),
                });
                s.active_mut().running = false;
            }
        }
        self.ping();
    }

    fn new_session(&self) {
        self.record("new_session");
        {
            let mut s = self.state.lock().unwrap();
            let id = format!("t{}", s.threads.len() + 1);
            let name = format!("thread {}", s.threads.len() + 1);
            s.threads.push(Thread {
                id: id.clone(),
                name,
                session_id: gen_id("tui"),
                messages: Vec::new(),
                event_log: ThreadEventLog::default(),
                running: false,
                last_result: None,
            });
            s.active_id = id;
        }
        self.ping();
    }

    fn set_active_thread(&self, id: String) {
        self.record("set_active_thread");
        {
            let mut s = self.state.lock().unwrap();
            if s.threads.iter().any(|t| t.id == id) {
                s.active_id = id;
            }
        }
        self.ping();
    }

    fn answer_question(&self, cycle_id: String, question_id: String, body: String) {
        self.record(&format!("answer_question:{cycle_id}:{question_id}:{body}"));
        self.ping();
    }

    fn list_main_chats(&self) -> BoxFuture<'static, anyhow::Result<Vec<MainChatSummary>>> {
        Box::pin(async move {
            Ok(vec![
                MainChatSummary {
                    session_id: "tui-demo-1".into(),
                    name: "Auth refactor".into(),
                    turns: 4,
                    thread_count: 2,
                    updated_at: crate::ui::chat_store::iso8601_utc(now_millis()),
                },
                MainChatSummary {
                    session_id: "tui-demo-2".into(),
                    name: "Repo audit".into(),
                    turns: 2,
                    thread_count: 1,
                    updated_at: crate::ui::chat_store::iso8601_utc(now_millis() - 86_400_000),
                },
            ])
        })
    }

    fn resume_chat(&self, _main_session_id: String) -> BoxFuture<'static, anyhow::Result<()>> {
        let state = self.state.clone();
        let tx = self.tx.clone();
        Box::pin(async move {
            {
                let mut s = state.lock().unwrap();
                if s.threads.iter().any(|t| t.running) {
                    return Err(anyhow::anyhow!("cannot resume while a thread is running"));
                }
                s.active_mut().messages.push(ChatMessage {
                    role: "assistant".into(),
                    content: "(resumed chat)".into(),
                });
            }
            let _ = tx.send(());
            Ok(())
        })
    }

    fn inspect_context(&self) -> BoxFuture<'static, anyhow::Result<Vec<ContextItem>>> {
        Box::pin(async move {
            Ok(vec![
                ContextItem {
                    ref_: "ctx://task-1/result".into(),
                    kind: "task-result".into(),
                    bytes: 482,
                    content: "Refactored auth into 3 focused modules.".into(),
                },
                ContextItem {
                    ref_: "ctx://memory/house-rules".into(),
                    kind: "memory".into(),
                    bytes: 128,
                    content: "Always run the test suite before handoff.".into(),
                },
            ])
        })
    }

    fn shutdown(&self) -> BoxFuture<'static, anyhow::Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}
