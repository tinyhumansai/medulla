//! The [`Runtime`] trait implementation for [`CoreRuntime`]: snapshotting,
//! subscription, the `instruct`-backed submit, and the core-runtime steering
//! hooks the serve protocol covers (`answer_question`, `cancel_task`,
//! `stream_state`). Hooks the protocol does *not* cover are left as no-ops, each
//! annotated with the serve frame that would back it in a later milestone.
//!
//! [`Runtime`]: crate::runtime::Runtime

use futures::future::BoxFuture;
use serde_json::json;
use tokio::sync::{broadcast, oneshot};

use crate::runtime::{ContextItem, Runtime, RuntimeSnapshot, StreamState, ThreadSummary};
use crate::ui::chat_store::{ChatMessage, MainChatSummary};
use crate::ui::events::TuiEvent;

use super::client::CoreRuntime;
use super::types::{Command, CoreError, REQUEST_TIMEOUT};

impl Runtime for CoreRuntime {
    fn describe(&self) -> String {
        let base = self.state.lock().unwrap().describe();
        format!("{base} @ {}", self.socket_path.display())
    }

    fn snapshot(&self) -> RuntimeSnapshot {
        let s = self.state.lock().unwrap();
        // One synthetic thread mirrors the single serve session (serve-protocol
        // §1 "one session per connection").
        let threads = vec![ThreadSummary {
            id: "core".into(),
            parent_id: None,
            name: "main".into(),
            running: s.running,
            turns: s.messages.len().div_ceil(2),
            running_tasks: s
                .harness
                .as_ref()
                .map(|h| h.tasks.iter().filter(|t| is_active(t.status)).count())
                .unwrap_or(0),
            attention: 0,
        }];
        RuntimeSnapshot {
            session_id: s.session_id.clone(),
            running: s.running,
            events: s.events.clone(),
            chat_events: s.chat_events.clone(),
            messages: s.messages.clone(),
            last_result: s.last_result.clone(),
            tracing: false,
            roster: Vec::new(),
            presence: Default::default(),
            sessions: Default::default(),
            tinyplace: None,
            async_mode: s.async_mode,
            threads,
            active_thread_id: "core".into(),
            harness: s.harness.clone(),
            replay_epoch: s.replay_epoch,
        }
    }

    fn subscribe(&self) -> broadcast::Receiver<()> {
        self.tx.subscribe()
    }

    fn submit(&self, input: String) -> BoxFuture<'static, anyhow::Result<()>> {
        let cmd_tx = self.cmd_tx.clone();
        let state = self.state.clone();
        let tx = self.tx.clone();
        Box::pin(async move {
            // Optimistically show the operator's turn (the serve stream carries
            // the cycle back, but not necessarily a user echo).
            {
                let mut s = state.lock().unwrap();
                s.messages.push(ChatMessage {
                    role: "user".into(),
                    content: input.clone(),
                });
                // Remember this turn so the wire echo of it (folded back via
                // `fold_event`) is recognised and not appended a second time.
                s.pending_user_echo = Some(input.clone());
                s.emit(TuiEvent::User {
                    body: input.clone(),
                });
            }
            let _ = tx.send(());

            let params = json!({ "message": input, "meta": { "origin": "submit" } });
            request(&cmd_tx, "instruct", params).await.map(|_| ())
        })
    }

    fn abort(&self) {
        // No per-cycle abort op exists; `stop` (drain=false) aborts the active
        // cycle (serve-protocol §4.6). Fire-and-forget, like the trait contract.
        let _ = self.cmd_tx.send(Command::Fire {
            op: "stop",
            params: json!({ "drain": false }),
        });
    }

    fn new_session(&self) {
        // No serve frame resets a session in place — serve owns the harness and
        // rehydrates from the host `sessions` port (serve-protocol §7). Clear the
        // local transcript so the view starts fresh; the durable session is
        // untouched.
        {
            let mut s = self.state.lock().unwrap();
            s.messages.clear();
            s.events.clear();
            s.chat_events.clear();
            s.last_result = None;
            s.running = false;
        }
        self.ping();
    }

    fn fork(&self, _name: Option<String>) -> String {
        // No-op: serve is one session per connection (serve-protocol §1); there
        // is no fork frame. Return the current session id unchanged.
        self.state.lock().unwrap().session_id.clone()
    }

    fn set_active_thread(&self, _id: String) {
        // No-op: single session per connection; no thread-switch frame exists.
    }

    fn list_main_chats(&self) -> BoxFuture<'static, anyhow::Result<Vec<MainChatSummary>>> {
        // No-op surface: chat history lives behind the host `sessions` port, not
        // a serve `req`. Returns empty until that port is hosted.
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn resume_chat(&self, _main_session_id: String) -> BoxFuture<'static, anyhow::Result<()>> {
        // No-op: serve resumes its own session from the `sessions` port on
        // respawn (serve-protocol §7); the host has no resume frame to send.
        Box::pin(async move { Ok(()) })
    }

    fn set_async_mode(&self, on: bool) -> bool {
        // Local flag only; no serve op backs it (mirrors the backend runtime).
        {
            self.state.lock().unwrap().async_mode = on;
        }
        self.ping();
        on
    }

    fn inspect_context(&self) -> BoxFuture<'static, anyhow::Result<Vec<ContextItem>>> {
        // No-op: `context` is a host-owned reverse-RPC port (serve→host,
        // serve-protocol §5.4), not something the host requests from serve.
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn shutdown(&self) -> BoxFuture<'static, anyhow::Result<()>> {
        let cmd_tx = self.cmd_tx.clone();
        Box::pin(async move {
            let (reply, rx) = oneshot::channel();
            if cmd_tx.send(Command::Shutdown { reply }).is_ok() {
                let _ = rx.await;
            }
            Ok(())
        })
    }

    // --- operator steering the serve protocol covers -------------------------

    fn answer_question(&self, cycle_id: String, question_id: String, body: String) {
        // `answer_question` (serve-protocol §4.2). Fire-and-forget: the `res`
        // only acks receipt. `taskId` is optional and not carried by the trait.
        let _ = self.cmd_tx.send(Command::Fire {
            op: "answer_question",
            params: json!({ "cycleId": cycle_id, "questionId": question_id, "body": body }),
        });
    }

    fn cancel_task(&self, cycle_id: String, task_id: String) {
        // `cancel_task` (serve-protocol §4.3). Fire-and-forget ack semantics.
        let _ = self.cmd_tx.send(Command::Fire {
            op: "cancel_task",
            params: json!({ "cycleId": cycle_id, "taskId": task_id }),
        });
    }

    fn stream_state(&self) -> Option<StreamState> {
        // The lossy event tap's health (serve-protocol §6): Live when the seq is
        // contiguous, Resyncing on a gap / reconnect, Stalled when unavailable.
        Some(self.state.lock().unwrap().stream_health())
    }
}

/// Whether a tracked-task status counts as an in-flight lane for the thread card.
fn is_active(status: crate::harness_contract::TrackedTaskStatus) -> bool {
    use crate::harness_contract::TrackedTaskStatus::*;
    matches!(status, Open | Active | Blocked)
}

/// Enqueue a correlated `req` and await its `res`, mapping the wire/transport
/// failures into `anyhow`. Applies the per-request timeout (serve-protocol §7);
/// `instruct` returns its receipt fast, so the bounded wait is safe.
async fn request(
    cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    op: &'static str,
    params: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let (reply, rx) = oneshot::channel();
    cmd_tx
        .send(Command::Request { op, params, reply })
        .map_err(|_| anyhow::anyhow!("core runtime is not connected"))?;
    let settled = tokio::time::timeout(REQUEST_TIMEOUT, rx)
        .await
        .map_err(|_| anyhow::anyhow!("{op} timed out"))?
        .map_err(|_| anyhow::anyhow!("core runtime dropped the request"))?;
    settled.map_err(|e: CoreError| anyhow::anyhow!("{op} failed: {e}"))
}

/// Whether the runtime has latched a fatal, unavailable state. Test/diagnostic
/// seam used by the module's unit tests.
#[cfg(test)]
pub(super) fn is_unavailable(rt: &CoreRuntime) -> bool {
    matches!(
        rt.state.lock().unwrap().conn,
        super::types::ConnState::Unavailable(_)
    )
}
