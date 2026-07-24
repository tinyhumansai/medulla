//! Mid-run input, and stopping a task the requester has given up on.

use crate::tinyplace::{TaskFrame, TaskFrameKind};

use super::super::providers::{self};
use super::super::types::DaemonRuntime;

impl DaemonRuntime {
    /// Stop a running task the requester has given up on.
    ///
    /// Two things are being freed, and the second is the one that bites: the
    /// harness stops working on an answer nobody will read, and the task's id
    /// stops being live. A responder refuses a second task whose id is already
    /// running, so an abandoned task holds its name — and unnamed tasks are
    /// named positionally, so `t1` recurs constantly — until its own timeout
    /// expires, which is far longer than the requester's.
    ///
    /// Acked either way. "I stopped it" and "there was nothing to stop" are both
    /// fine outcomes for the requester, who is no longer waiting regardless.
    pub(super) async fn handle_abort(&self, from: String, frame: TaskFrame) {
        let key = Self::task_key(&from, &frame.task_id);
        let aborted = {
            let running = self.inner.running.lock().unwrap();
            match running.get(&key) {
                Some(task) => {
                    // A correlationId mismatch means a different dispatch reused
                    // the taskId — the same guard `handle_input` makes, and for
                    // a worse failure. Task ids recur by construction: they are
                    // positional per `delegate_tasks` call, and the hub's
                    // uniquifying suffix restarts from zero when it restarts. So
                    // a late abort for a task that already finished can name a
                    // live one, and cancelling that is silent and total.
                    let mismatch = matches!(
                        (&frame.correlation_id, &task.correlation_id),
                        (Some(a), Some(b)) if a != b
                    );
                    if mismatch {
                        false
                    } else {
                        task.abort.abort();
                        true
                    }
                }
                None => false,
            }
        };
        let detail = if aborted {
            "task aborted"
        } else {
            "no matching running task to abort"
        };
        self.log(&format!("task {} ⨯ {detail}", frame.task_id));
        self.reply(
            &from,
            TaskFrameKind::Ack,
            &frame.task_id,
            detail,
            frame.correlation_id.as_deref(),
            None,
        )
        .await;
    }

    /// Deliver an `input` frame to the matching running task (or reject it).
    pub(super) async fn handle_input(&self, from: String, frame: TaskFrame) {
        let key = Self::task_key(&from, &frame.task_id);
        let no_match = (
            TaskFrameKind::Ack,
            "no matching running task for input",
            self.inner.config.default_provider,
        );
        let (kind, text, harness) = {
            let mut running = self.inner.running.lock().unwrap();
            match running.get_mut(&key) {
                Some(task) => {
                    // A correlationId mismatch means a different dispatch reused
                    // the taskId — treat as no match rather than crossing sessions.
                    let mismatch = matches!(
                        (&frame.correlation_id, &task.correlation_id),
                        (Some(a), Some(b)) if a != b
                    );
                    if mismatch {
                        no_match
                    } else if !providers::supports_stdin(task.provider) {
                        // The child has a null stdin; buffering would silently
                        // discard the guidance, so reject it honestly instead.
                        (
                            TaskFrameKind::Error,
                            "provider does not accept mid-run input",
                            task.provider,
                        )
                    } else {
                        match &task.stdin {
                            Some(stdin) => {
                                let _ = stdin.send(frame.text.clone());
                            }
                            None => task.pending_input.push(frame.text.clone()),
                        }
                        (TaskFrameKind::Ack, "input received", task.provider)
                    }
                }
                None => no_match,
            }
        };
        self.reply(
            &from,
            kind,
            &frame.task_id,
            text,
            frame.correlation_id.as_deref(),
            Some(harness),
        )
        .await;
    }
}
