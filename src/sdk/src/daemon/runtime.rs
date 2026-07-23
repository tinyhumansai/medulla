//! [`DaemonRuntime`] lifecycle: construction and test overrides, fire-and-forget
//! dispatch and idle/shutdown coordination, controller bookkeeping, and the
//! encrypted reply helpers. The frame- and task-handling half of the state
//! machine lives in [`super::task_loop`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::{Mutex as TokioMutex, Notify, Semaphore};

use ::tinyplace::auth::timestamp;

use crate::tinyplace::{EncodeFrameInput, HarnessProvider, TaskFrame, TaskFrameKind};

use super::providers::{Abort, RunTaskFn};
use super::types::{DaemonConfig, DaemonRuntime, Inner, LogFn, NowFn, SendFn};

impl DaemonRuntime {
    /// Build a runtime from `config`, an executor (`run_task`), and a
    /// lock-serialized `send`.
    pub fn new(config: DaemonConfig, run_task: RunTaskFn, send: SendFn) -> Self {
        let concurrency = config.concurrency.max(1);
        DaemonRuntime {
            inner: Arc::new(Inner {
                config,
                run_task,
                send,
                now: Arc::new(system_now_ms),
                log: None,
                running: StdMutex::new(HashMap::new()),
                controllers: StdMutex::new(HashMap::new()),
                next_controller_id: AtomicU64::new(0),
                admitted: AtomicUsize::new(0),
                slots: Semaphore::new(concurrency),
                inflight_count: AtomicUsize::new(0),
                inflight_idle: Notify::new(),
                capabilities: TokioMutex::new(None),
            }),
        }
    }

    /// Override the clock (tests).
    pub fn with_now(self, now: NowFn) -> Self {
        // Only valid before any dispatch; rebuild the inner with the new clock.
        let inner = Arc::try_unwrap(self.inner)
            .unwrap_or_else(|_| panic!("with_now must be called before cloning/dispatch"));
        DaemonRuntime {
            inner: Arc::new(Inner { now, ..inner }),
        }
    }

    /// Attach a diagnostics sink (tests/serve).
    pub fn with_log(self, log: LogFn) -> Self {
        let inner = Arc::try_unwrap(self.inner)
            .unwrap_or_else(|_| panic!("with_log must be called before cloning/dispatch"));
        DaemonRuntime {
            inner: Arc::new(Inner {
                log: Some(log),
                ..inner
            }),
        }
    }

    /// Number of tasks currently executing.
    pub fn active_count(&self) -> usize {
        self.inner
            .config
            .concurrency
            .max(1)
            .saturating_sub(self.inner.slots.available_permits())
    }

    /// Fire-and-forget dispatch of one inbound message. Never panics to the
    /// caller; the work runs on a spawned task tracked by [`DaemonRuntime::idle`].
    pub fn handle_message(&self, from: String, text: String, frame: Option<TaskFrame>) {
        self.inner.inflight_count.fetch_add(1, Ordering::SeqCst);
        let this = self.clone();
        tokio::spawn(async move {
            match frame {
                Some(frame) => this.handle_frame(from, frame).await,
                None => this.handle_plain_text(from, text).await,
            }
            if this.inner.inflight_count.fetch_sub(1, Ordering::SeqCst) == 1 {
                this.inner.inflight_idle.notify_waiters();
            }
        });
    }

    /// Resolve once every dispatched message has fully settled (used by `--once`).
    pub async fn idle(&self) {
        loop {
            if self.inner.inflight_count.load(Ordering::SeqCst) == 0 {
                return;
            }
            let notified = self.inner.inflight_idle.notified();
            if self.inner.inflight_count.load(Ordering::SeqCst) == 0 {
                return;
            }
            notified.await;
        }
    }

    /// Abort every in-flight run for clean shutdown.
    pub fn shutdown(&self) {
        for abort in self.inner.controllers.lock().unwrap().values() {
            abort.abort();
        }
    }

    /// Emit a diagnostic line if a sink is attached.
    pub(super) fn log(&self, line: &str) {
        if let Some(log) = &self.inner.log {
            log(line);
        }
    }

    /// The map key for a running task: `sender + taskId`.
    pub(super) fn task_key(from: &str, task_id: &str) -> String {
        format!("{from} {task_id}")
    }

    /// Register an abort handle and return its controller id.
    pub(super) fn register_controller(&self, abort: Abort) -> u64 {
        let id = self.inner.next_controller_id.fetch_add(1, Ordering::SeqCst);
        self.inner.controllers.lock().unwrap().insert(id, abort);
        id
    }

    /// Drop a previously registered abort handle.
    pub(super) fn unregister_controller(&self, id: u64) {
        self.inner.controllers.lock().unwrap().remove(&id);
    }

    /// Send a frame with no token usage attached.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn reply(
        &self,
        to: &str,
        kind: TaskFrameKind,
        task_id: &str,
        text: &str,
        correlation: Option<&str>,
        harness: Option<HarnessProvider>,
    ) {
        self.reply_with_usage(to, kind, task_id, text, correlation, harness, None)
            .await;
    }

    /// Encode and send a task frame, optionally carrying token usage.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn reply_with_usage(
        &self,
        to: &str,
        kind: TaskFrameKind,
        task_id: &str,
        text: &str,
        correlation: Option<&str>,
        harness: Option<HarnessProvider>,
        usage: Option<crate::tinyplace::TokenUsage>,
    ) {
        let body = crate::tinyplace::encode_task_frame_with_usage(
            EncodeFrameInput {
                kind,
                task_id: task_id.to_string(),
                text: text.to_string(),
                ts: timestamp(),
                correlation_id: correlation.map(str::to_string),
                harness,
                provider: None,
                model: None,
            },
            usage,
        );
        // Narrate the terminal frames only. Status and ack are throttled chatter
        // whose whole point is that nobody reads them one by one; a reply or an
        // error is the thing a peer waited for, and the one worth being able to
        // prove was sent.
        if matches!(kind, TaskFrameKind::Reply | TaskFrameKind::Error) {
            self.log(&format!(
                "task {task_id} → {to} {} · {} bytes on the wire, {} chars: {}",
                kind.as_str(),
                body.len(),
                text.chars().count(),
                crate::logging::preview(text),
            ));
        }
        self.send_raw(to, &body).await;
    }

    /// The lowest-level send: hand `body` to the transport for `to`.
    pub(super) async fn send_raw(&self, to: &str, body: &str) {
        (self.inner.send)(to.to_string(), body.to_string()).await;
    }
}

/// The default wall clock: epoch ms, saturating to 0 on error. Delegates to the
/// shared [`crate::clock`] helper.
fn system_now_ms() -> i64 {
    crate::clock::now_millis()
}
