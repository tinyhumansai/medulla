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
use super::types::{DaemonConfig, DaemonRuntime, FrameAttachments, Inner, LogFn, NowFn, SendFn};

impl DaemonRuntime {
    /// Build a runtime from `config`, an executor (`run_task`), and a
    /// lock-serialized `send`.
    pub fn new(config: DaemonConfig, run_task: RunTaskFn, send: SendFn) -> Self {
        let concurrency = config.concurrency.max(1);
        let accessible_dirs = config.accessible_dirs.clone();
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
                accessible_dirs: StdMutex::new(accessible_dirs),
                sessions: crate::sessions::SessionRegistry::default(),
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
        // A screen message is never a prompt. The plain-text path types whatever
        // it is given into a harness, so a `medulla.screen.v1` body reaching it
        // is executed rather than ignored — a subscribe arriving as an
        // instruction to claude. Callers that can actually serve one classify it
        // before getting here; this refuses it for every caller that cannot, so
        // the mistake is not one each new drain has to remember to avoid.
        if crate::tinyplace::parse_screen_message(&text).is_some() {
            self.log(&format!(
                "screen: ignored a screen message from {from} — this daemon serves no watchable sessions"
            ));
            return;
        }
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

    /// Replace the workspace roots advertised to orchestrators.
    ///
    /// Clears the cached capability probe so the next request observes the
    /// change immediately. The task working directory itself is unchanged.
    pub async fn set_accessible_dirs(&self, dirs: Vec<String>) {
        *self.inner.accessible_dirs.lock().unwrap() = dirs;
        *self.inner.capabilities.lock().await = None;
    }

    /// Emit a diagnostic line if a sink is attached.
    pub(super) fn log(&self, line: &str) {
        if let Some(log) = &self.inner.log {
            log(line);
        }
    }

    /// The map key for a running task: `sender + taskId`.
    /// The worker-local session running `task_id` for `from`, if any.
    ///
    /// The key is `(authenticated sender, task id)`, which is what makes this
    /// safe to expose: a peer can only ever resolve a task it dispatched
    /// itself, so screen subscriptions need no ownership check of their own.
    /// `None` once the task settles — the record is removed then, so a stream
    /// keyed on it ends with the work rather than outliving it.
    pub fn session_for_task(&self, from: &str, task_id: &str) -> Option<String> {
        self.inner
            .running
            .lock()
            .unwrap()
            .get(&Self::task_key(from, task_id))
            .and_then(|task| task.session_id.clone())
    }

    /// Record the session an executor opened for a running task.
    pub(super) fn record_task_session(&self, key: &str, session_id: String) {
        if let Some(task) = self.inner.running.lock().unwrap().get_mut(key) {
            task.session_id = Some(session_id);
        }
    }

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

    /// Send a frame carrying token usage and nothing else.
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
        self.reply_with(
            to,
            kind,
            task_id,
            text,
            correlation,
            harness,
            FrameAttachments {
                usage,
                ..Default::default()
            },
        )
        .await;
    }

    /// Encode and send a task frame with its optional attachments.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn reply_with(
        &self,
        to: &str,
        kind: TaskFrameKind,
        task_id: &str,
        text: &str,
        correlation: Option<&str>,
        harness: Option<HarnessProvider>,
        attachments: FrameAttachments,
    ) {
        let body = crate::tinyplace::encode_task_frame_with_work(
            EncodeFrameInput {
                kind,
                task_id: task_id.to_string(),
                text: text.to_string(),
                ts: timestamp(),
                correlation_id: correlation.map(str::to_string),
                harness,
                provider: None,
                model: None,
                workflow: None,
                // Inbound-only, like `provider`, `model`, and `workflow`: this
                // builds the worker's *responses*, and continuity is the
                // sender's decision, not something a reply restates.
                conversation: None,
            },
            attachments.usage,
            attachments.work,
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
