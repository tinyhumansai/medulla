//! Serving inbound `medulla.screen.v1` messages: resolving which session a
//! subscriber may watch, and starting or stopping the stream that answers.
//!
//! This is the half of the protocol the worker *receives*. The vocabulary is
//! deliberately tiny — subscribe, unsubscribe, acknowledge, and an explicit kill.
//! There is no message that can type into or resize a session. Kill is resolved
//! through the same task ownership check as subscribe, so a controller can stop
//! only a harness running work that controller dispatched.
//!
//! Which leaves one question, and the answer is structural rather than a check.
//! A subscription names a **task**, and the daemon's running-task record is
//! keyed by `(authenticated sender, task id)` — so resolving one at all proves
//! the caller dispatched it. A peer cannot name another's task, because the key
//! it would need includes an identity it does not have. There is no ownership
//! field to keep in sync and no comparison to get wrong.
//!
//! Addressing by task rather than by session is sound because a session serves
//! one task at a time: `claim_idle` refuses a busy session and marks the one it
//! returns, under a single lock, and discrete work always opens a fresh
//! session. The record also disappears when the task settles, so a stream ends
//! with the work rather than outliving it.

use medulla::daemon::{DaemonRuntime, LogFn, SendFn};
use medulla::tinyplace::ScreenMessage;

use super::super::pty::PtyManager;
use super::sampler::StreamRegistry;

/// Serves screen subscriptions for one worker.
///
/// Holds the live streams, so dropping it drops them.
pub struct ScreenRouter {
    sessions: PtyManager,
    runtime: DaemonRuntime,
    registry: StreamRegistry,
    send: SendFn,
    log: Option<LogFn>,
}

impl ScreenRouter {
    /// Build a router over the worker's live sessions, task records and
    /// encrypted transport.
    pub fn new(sessions: PtyManager, runtime: DaemonRuntime, send: SendFn) -> Self {
        ScreenRouter {
            sessions,
            runtime,
            registry: StreamRegistry::new(),
            send,
            log: None,
        }
    }

    /// Narrate refusals and subscriptions to `log`.
    ///
    /// Worth having: a refused subscribe is otherwise indistinguishable from a
    /// message that never arrived, and those call for opposite investigations.
    pub fn with_log(mut self, log: LogFn) -> Self {
        self.log = Some(log);
        self
    }

    fn log(&self, line: &str) {
        if let Some(log) = &self.log {
            log(line);
        }
    }

    /// How many tasks are currently being streamed.
    pub fn active(&self) -> usize {
        self.registry.len()
    }

    /// Handle one screen message from the authenticated peer `from`.
    ///
    /// A task that cannot be resolved is answered with silence. Replying would
    /// distinguish "not yours" from "no such task", which tells an unauthorized
    /// caller what is running on this machine.
    pub fn handle(&mut self, from: &str, message: ScreenMessage) {
        // Streams whose task has settled leave a finished sampler behind; clear
        // them so a long-lived worker's registry tracks reality.
        self.registry.prune();

        match message {
            ScreenMessage::Subscribe {
                task_id,
                max_fps,
                resync,
            } => self.subscribe(from, &task_id, max_fps, resync),
            ScreenMessage::Unsubscribe { task_id } => {
                // Authorized against the stream's own subscriber, not against
                // the task still running. Both stop an impostor cancelling
                // someone else's stream, but only this one still works once the
                // task has finished — and that is precisely when a viewer lets
                // go of it.
                if self.registry.unsubscribe_for(from, &task_id) {
                    self.log(&format!("screen: {from} unsubscribed from {task_id}"));
                }
            }
            ScreenMessage::Kill {
                task_id,
                correlation_id,
            } => {
                if !self.runtime.terminate_task(from, &task_id, &correlation_id) {
                    self.log(&format!(
                        "screen: refused kill from {from} on {task_id} — no such running task for this sender"
                    ));
                    return;
                }
                self.registry.unsubscribe(&task_id);
                self.log(&format!("screen: {from} killed {task_id}"));
            }
            // The sampler chains from the last frame it actually sent, so an
            // acknowledgement is not needed to decide what to send next. Kept in
            // the protocol as the viewer's liveness signal and accepted here so
            // it is never mistaken for an unknown message.
            ScreenMessage::Ack { .. } => {}
            // We are the sender; a frame arriving here is either a loopback or a
            // peer that has the protocol backwards.
            ScreenMessage::Frame(_) => {}
        }
    }

    /// Start, restart, or leave alone the stream for `task_id`.
    fn subscribe(&mut self, from: &str, task_id: &str, max_fps: u8, resync: bool) {
        let Some(session_id) = self.runtime.session_for_task(from, task_id) else {
            self.log(&format!(
                "screen: refused {from} on {task_id} — no such running task for this sender"
            ));
            return;
        };
        // A headless run has a record but no pty, so there is nothing to watch
        // even though the task is real.
        if self.sessions.row(&session_id).is_none() {
            self.log(&format!(
                "screen: refused {from} on {task_id} — that task has no watchable session"
            ));
            return;
        }
        // An already-running stream is left alone unless the viewer says it has
        // lost track. Restarting on every subscribe would force a full frame
        // each time, which is the expensive thing this protocol exists to avoid.
        if self.registry.contains(task_id) && !resync {
            return;
        }
        // The stream's own stopping condition, resolved the same way this
        // subscribe was: it ends when `(from, task_id)` stops naming a running
        // task, which is what keeps it from outliving its task and sampling
        // whatever claims the session next.
        let is_live = {
            let runtime = self.runtime.clone();
            let from = from.to_string();
            let task_id = task_id.to_string();
            std::sync::Arc::new(move || runtime.session_for_task(&from, &task_id).is_some())
        };
        self.registry.subscribe(
            &self.sessions,
            super::sampler::StreamSpec {
                task_id: task_id.to_string(),
                session_id: session_id.clone(),
                subscriber: from.to_string(),
                max_fps,
                is_live,
            },
            self.send.clone(),
        );
        self.log(&format!(
            "screen: streaming {task_id} (session {session_id}) to {from} at {max_fps} fps{}",
            if resync { " (resync)" } else { "" }
        ));
    }

    /// Drop every subscription — used on shutdown.
    pub fn clear(&mut self) {
        self.registry.clear();
    }
}
