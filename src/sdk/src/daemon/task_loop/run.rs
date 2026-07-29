//! Executing one delegated task: slots, status forwarding, fallback.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::harness_work::{WorkFold, WorkSnapshot};
use crate::tinyplace::{TaskFrame, TaskFrameKind};

use super::super::mappers;
use super::super::providers::{Abort, RunTaskOptions};
use super::super::status::{status_detail, work_detail};
use super::super::types::{DaemonRuntime, FrameAttachments, RunningTask};

impl DaemonRuntime {
    /// Admit, execute, and reply to a `task` frame, forwarding throttled status.
    pub(super) async fn handle_task(&self, from: String, frame: TaskFrame) {
        let correlation = frame.correlation_id.clone();
        let provider = match self.select_provider(frame.provider) {
            Some(provider) => provider,
            None => {
                let offered = self
                    .inner
                    .config
                    .providers
                    .iter()
                    .map(|p| p.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                let offered = if offered.is_empty() {
                    "(none)".to_string()
                } else {
                    offered
                };
                let requested = frame
                    .provider
                    .map(|p| format!(" for requested \"{}\"", p.as_str()))
                    .unwrap_or_default();
                self.reply(
                    &from,
                    TaskFrameKind::Error,
                    &frame.task_id,
                    &format!("no available provider{requested}; daemon offers: {offered}"),
                    correlation.as_deref(),
                    None,
                )
                .await;
                return;
            }
        };

        if self.inner.admitted.load(Ordering::SeqCst) >= self.inner.config.max_pending {
            self.reply(
                &from,
                TaskFrameKind::Error,
                &frame.task_id,
                &format!(
                    "daemon at capacity ({} pending tasks); retry later",
                    self.inner.config.max_pending
                ),
                correlation.as_deref(),
                Some(provider),
            )
            .await;
            return;
        }

        let key = Self::task_key(&from, &frame.task_id);
        // An active duplicate (same sender + taskId) must not clobber the record.
        if self.inner.running.lock().unwrap().contains_key(&key) {
            self.reply(
                &from,
                TaskFrameKind::Error,
                &frame.task_id,
                &format!("task {} is already running for this sender", frame.task_id),
                correlation.as_deref(),
                Some(provider),
            )
            .await;
            return;
        }
        // Register BEFORE acking so a racing `input` frame finds the record.
        let abort = Abort::new();
        self.inner.running.lock().unwrap().insert(
            key.clone(),
            RunningTask {
                provider,
                abort: abort.clone(),
                correlation_id: correlation.clone(),
                stdin: None,
                pending_input: Vec::new(),
                session_id: None,
            },
        );
        self.inner.admitted.fetch_add(1, Ordering::SeqCst);
        let controller_id = self.register_controller(abort.clone());

        self.reply(
            &from,
            TaskFrameKind::Ack,
            &frame.task_id,
            "task accepted",
            correlation.as_deref(),
            Some(provider),
        )
        .await;

        self.log(&format!("task {} → {}", frame.task_id, provider.as_str()));

        // Slot-limited execution (FIFO via the semaphore).
        let permit = self
            .inner
            .slots
            .acquire()
            .await
            .expect("semaphore is never closed");

        // What the harness is working on, folded from its own stream. Shared
        // with the reply below so the terminal frame carries the final picture
        // even when the last change was throttled away.
        let work = Arc::new(Mutex::new(WorkFold::new()));

        // Status frames: onEvent (sync) throttles + forwards details over a
        // channel; a consumer sends them in order before the final reply. Each
        // frame carries the current work snapshot, so a dropped update is
        // corrected by the next one rather than lost.
        let (status_tx, mut status_rx) =
            mpsc::unbounded_channel::<(String, Option<WorkSnapshot>)>();
        let on_event = {
            let now = self.inner.now.clone();
            let throttle = self.inner.config.status_throttle_ms;
            let mut last_status_at: i64 = i64::MIN;
            let status_tx = status_tx.clone();
            let work = work.clone();
            Box::new(move |semantic: &mappers::HarnessSemanticEvent| {
                // Fold first, unconditionally: throttling governs how often the
                // peer is told, never what this worker knows.
                let mut fold = work.lock().expect("work fold lock");
                let changed = fold.apply(
                    &semantic.event.kind,
                    &semantic.event.payload,
                    semantic.timestamp_ms,
                );
                // A todo write or a sub-agent spawn has no status wording of its
                // own; describing the work is what makes it visible at all.
                let detail = match status_detail(&semantic.event) {
                    Some(detail) => detail,
                    None if changed => match work_detail(fold.snapshot()) {
                        Some(detail) => detail,
                        None => return,
                    },
                    None => return,
                };
                let snapshot = fold.snapshot().clone();
                drop(fold);
                let current = now();
                if current.saturating_sub(last_status_at) < throttle {
                    return;
                }
                last_status_at = current;
                let _ = status_tx.send((detail, Some(snapshot)));
            }) as Box<dyn FnMut(&mappers::HarnessSemanticEvent) + Send>
        };
        drop(status_tx);

        let on_stdin = {
            let this = self.clone();
            let key = key.clone();
            Box::new(move |tx: mpsc::UnboundedSender<String>| {
                let mut running = this.inner.running.lock().unwrap();
                if let Some(record) = running.get_mut(&key) {
                    for buffered in record.pending_input.drain(..) {
                        let _ = tx.send(buffered);
                    }
                    record.stdin = Some(tx);
                }
            }) as Box<dyn FnOnce(mpsc::UnboundedSender<String>) + Send>
        };

        // The conversation this task belongs to, if it named one. Scoped to the
        // *authenticated* sender rather than taken bare from the frame: the
        // frame body cannot be trusted to name its own author, so without this
        // one peer could name another's conversation and resume into a session
        // holding someone else's context.
        let session_key = frame.conversation.as_deref().map(|conversation| {
            crate::sessions::SessionKey::new(format!("{from}/{conversation}"), provider)
        });
        // Held for the rest of this turn, past the `plan()` below and through
        // `run_task` — including the `on_session` callback that records the
        // binding once the harness opens one. Without it, two frames naming
        // the same conversation could both `plan()` before either recorded a
        // session: the first pair would each open their own session and race
        // to bind it (whichever `on_session` fires last silently wins, and the
        // other harness session is orphaned); a later pair could both resume
        // the *same* now-bound session concurrently, interleaving two turns on
        // one harness. `acquire_turn` is a no-op (`None`) for anything but
        // `Unbound`, so a task frame with no named conversation never
        // serializes against unrelated work.
        let turn_guard = match &session_key {
            Some(key) => {
                self.inner
                    .sessions
                    .acquire_turn(key, crate::sessions::SessionClass::Unbound)
                    .await
            }
            None => None,
        };
        // An `Unbound` class because these turns are a conversation, not
        // discrete work — that is the whole of what the sender opted into. The
        // registry still declines to resume a provider that cannot
        // (`opencode`), which costs continuity but never correctness.
        let plan = session_key.as_ref().map(|key| {
            self.inner
                .sessions
                .plan(key, crate::sessions::SessionClass::Unbound)
        });
        let resume_session_id = plan
            .as_ref()
            .and_then(|plan| plan.resume_session_id.clone());

        // Reported as soon as the executor opens a session, not with the
        // result: the point of knowing it is to watch the task while it runs.
        let on_session = {
            let this = self.clone();
            let key = key.clone();
            let session_key = session_key.clone();
            Box::new(move |session_id: String| {
                // Bind before the turn finishes. A turn that opens a session and
                // then times out has still moved the conversation on, and the
                // next instruction has to resume *that* session rather than
                // starting a third one the operator never sees.
                if let Some(session_key) = &session_key {
                    this.inner.sessions.record(session_key, session_id.clone());
                }
                this.record_task_session(&key, session_id);
            }) as Box<dyn FnOnce(String) + Send>
        };

        let options = RunTaskOptions {
            // The *authenticated* sender, never anything from the frame body: a
            // frame cannot be trusted to name its own author. This says *whose*
            // the run is; `session_class` separately says whether it may share
            // context with the sender's other work.
            conversation: from.clone(),
            // A task frame is discrete work, so it gets its own session and can
            // see nothing of the sender's other tasks — `Bounded` even when the
            // frame names a conversation, because this field only chooses
            // between "a fresh session" and "whichever of the sender's sessions
            // is idle", and the latter cannot tell two of that sender's
            // conversations apart. Continuity, when a sender asks for it,
            // arrives through `resume_session_id` below, which names one
            // specific prior session scoped to `{sender}/{conversation}`.
            session_class: crate::sessions::SessionClass::Bounded,
            resume_session_id,
            provider,
            prompt: frame.text.clone(),
            cwd: self.inner.config.workspace.clone(),
            // Per-task, like the model and provider hints below. A review turn
            // asks for the restricted workflow tools here, which is the only
            // channel that reaches the MCP server the harness spawns.
            env: super::with_tool_mode(self.inner.config.env.clone(), frame.tool_mode.as_deref()),
            timeout_ms: self.inner.config.task_timeout_ms,
            // Per-task model hint (parallels the per-task `provider`): honor the
            // orchestrator's requested model, falling back to the daemon default.
            model: frame
                .model
                .clone()
                .or_else(|| self.inner.config.model.clone()),
            agent: self.inner.config.agent.clone(),
            extra_args: self.inner.config.extra_args.clone(),
            skip_permissions: self.inner.config.skip_permissions,
            abort: abort.clone(),
            router: self.inner.config.router.clone(),
            on_event: Some(on_event),
            on_stdin: Some(on_stdin),
            on_session: Some(on_session),
        };

        // Consume status details in order while the task runs.
        let status_consumer = {
            let this = self.clone();
            let from = from.clone();
            let task_id = frame.task_id.clone();
            let correlation = correlation.clone();
            tokio::spawn(async move {
                while let Some((detail, snapshot)) = status_rx.recv().await {
                    this.reply_with(
                        &from,
                        TaskFrameKind::Status,
                        &task_id,
                        &detail,
                        correlation.as_deref(),
                        Some(provider),
                        FrameAttachments {
                            usage: None,
                            work: snapshot,
                        },
                    )
                    .await;
                }
            })
        };

        let result = (self.inner.run_task)(options).await;
        // Released only now: the guard must outlive the `on_session` callback
        // above, which is the thing recording the binding the next queued turn
        // will plan against.
        drop(turn_guard);
        // The task future is dropped here, dropping its on_event (and its status
        // sender); the consumer then drains and ends.
        let _ = status_consumer.await;

        match result {
            Ok(run) => {
                // The reply carries the finished picture — the completed todo
                // list, every sub-agent, every file touched — so a caller that
                // only ever reads terminal frames still gets the whole story.
                let final_work = work.lock().expect("work fold lock").snapshot().clone();
                self.reply_with(
                    &from,
                    TaskFrameKind::Reply,
                    &frame.task_id,
                    &run.reply,
                    correlation.as_deref(),
                    Some(provider),
                    FrameAttachments {
                        usage: run.usage,
                        work: Some(final_work),
                    },
                )
                .await;
                // The captured turn content, as it was read out of the harness's
                // own transcript. Logged next to the send below so "the harness
                // answered" and "the peer was told" stop being one fact: an
                // empty reply here and a failed send there look identical from
                // the outside, and only one of them is the harness's fault.
                self.log(&format!(
                    "task {} ✓ ({} events) · captured {} chars: {}",
                    frame.task_id,
                    run.events,
                    run.reply.chars().count(),
                    crate::logging::preview(&run.reply),
                ));
            }
            Err(message) => {
                self.reply(
                    &from,
                    TaskFrameKind::Error,
                    &frame.task_id,
                    &message,
                    correlation.as_deref(),
                    Some(provider),
                )
                .await;
                self.log(&format!("task {} ✗ {message}", frame.task_id));
            }
        }

        drop(permit);
        self.inner.running.lock().unwrap().remove(&key);
        self.unregister_controller(controller_id);
        self.inner.admitted.fetch_sub(1, Ordering::SeqCst);
    }

    /// Run a plain-text DM through the default provider, replying with raw text.
    pub(in crate::daemon) async fn handle_plain_text(&self, from: String, text: String) {
        let provider = self.inner.config.default_provider;
        if !self.inner.config.providers.contains(&provider) {
            self.send_raw(&from, "No coding agent is available on this daemon.")
                .await;
            return;
        }
        if self.inner.admitted.load(Ordering::SeqCst) >= self.inner.config.max_pending {
            self.send_raw(
                &from,
                &format!(
                    "Daemon at capacity ({} pending tasks); retry later.",
                    self.inner.config.max_pending
                ),
            )
            .await;
            return;
        }
        let abort = Abort::new();
        let controller_id = self.register_controller(abort.clone());
        self.inner.admitted.fetch_add(1, Ordering::SeqCst);

        let permit = self
            .inner
            .slots
            .acquire()
            .await
            .expect("semaphore is never closed");
        self.log(&format!("plaintext DM → {}", provider.as_str()));
        let options = RunTaskOptions {
            conversation: from.clone(),
            // A conversational message continues the sender's session — that is
            // what makes a DM a conversation rather than a series of unrelated
            // one-shots.
            session_class: crate::sessions::SessionClass::Unbound,
            resume_session_id: None,
            provider,
            prompt: text,
            cwd: self.inner.config.workspace.clone(),
            env: self.inner.config.env.clone(),
            timeout_ms: self.inner.config.task_timeout_ms,
            model: self.inner.config.model.clone(),
            agent: self.inner.config.agent.clone(),
            extra_args: self.inner.config.extra_args.clone(),
            skip_permissions: self.inner.config.skip_permissions,
            abort,
            router: self.inner.config.router.clone(),
            on_event: None,
            on_stdin: None,
            on_session: None,
        };
        let result = (self.inner.run_task)(options).await;
        match result {
            Ok(run) => self.send_raw(&from, &run.reply).await,
            Err(message) => {
                self.send_raw(&from, &format!("Task failed: {message}"))
                    .await
            }
        }
        drop(permit);
        self.unregister_controller(controller_id);
        self.inner.admitted.fetch_sub(1, Ordering::SeqCst);
    }
}
