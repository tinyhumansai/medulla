//! The frame- and task-handling half of [`DaemonRuntime`]: routing decoded task
//! frames, the cached capability probe, mid-run input delivery, the slot-limited
//! task execution with throttled status forwarding, plain-text fallback, and
//! provider selection. Lifecycle/dispatch/reply glue lives in [`super::runtime`].

use std::sync::atomic::Ordering;

use tokio::sync::mpsc;

use crate::tinyplace::{AgentCapabilities, HarnessProvider, TaskFrame, TaskFrameKind};

use super::capabilities::{probe_capabilities, ProbeOptions};
use super::mappers;
use super::providers::{self, Abort, RunTaskOptions};
use super::status::status_detail;
use super::types::{DaemonRuntime, RunningTask, DEFAULT_CAPABILITY_TIMEOUT_MS};

impl DaemonRuntime {
    /// Route a decoded task frame to its handler; responses are ignored.
    pub(super) async fn handle_frame(&self, from: String, frame: TaskFrame) {
        match frame.kind {
            TaskFrameKind::Task => self.handle_task(from, frame).await,
            TaskFrameKind::Input => self.handle_input(from, frame).await,
            TaskFrameKind::Capabilities => self.handle_capabilities(from, frame).await,
            // status/reply/error/ack/capabilities_result are responses; ignore.
            _ => {}
        }
    }

    /// Answer a `capabilities` probe with the cached [`AgentCapabilities`].
    async fn handle_capabilities(&self, from: String, frame: TaskFrame) {
        let capabilities = self.get_capabilities().await;
        let text = serde_json::to_string(&capabilities).unwrap_or_else(|_| "{}".to_string());
        self.reply(
            &from,
            TaskFrameKind::CapabilitiesResult,
            &frame.task_id,
            &text,
            frame.correlation_id.as_deref(),
            Some(self.inner.config.default_provider),
        )
        .await;
    }

    /// Return the capabilities, probing once and caching the result.
    async fn get_capabilities(&self) -> AgentCapabilities {
        // Single cached probe shared across askers: holding the async mutex across
        // the probe means concurrent callers wait for the one run, then serve from
        // cache. Deliberately NOT counted against maxPending.
        let mut guard = self.inner.capabilities.lock().await;
        if let Some(capabilities) = guard.as_ref() {
            return capabilities.clone();
        }
        let provider = self
            .select_provider(None)
            .unwrap_or(self.inner.config.default_provider);
        self.log(&format!("capability probe → {}", provider.as_str()));
        let abort = Abort::new();
        let controller_id = self.register_controller(abort.clone());
        // Compete for the concurrency budget like a task.
        let permit = self
            .inner
            .slots
            .acquire()
            .await
            .expect("semaphore is never closed");
        let capabilities = probe_capabilities(ProbeOptions {
            provider,
            run_task: self.inner.run_task.clone(),
            workspace: self.inner.config.workspace.clone(),
            env: self.inner.config.env.clone(),
            providers: self.inner.config.providers.clone(),
            timeout_ms: self
                .inner
                .config
                .capability_timeout_ms
                .or(Some(DEFAULT_CAPABILITY_TIMEOUT_MS)),
            model: self.inner.config.model.clone(),
            agent: self.inner.config.agent.clone(),
            skip_permissions: self.inner.config.skip_permissions,
            abort,
        })
        .await;
        drop(permit);
        self.unregister_controller(controller_id);
        *guard = Some(capabilities.clone());
        capabilities
    }

    /// Deliver an `input` frame to the matching running task (or reject it).
    async fn handle_input(&self, from: String, frame: TaskFrame) {
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

    /// Admit, execute, and reply to a `task` frame, forwarding throttled status.
    async fn handle_task(&self, from: String, frame: TaskFrame) {
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
                correlation_id: correlation.clone(),
                stdin: None,
                pending_input: Vec::new(),
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

        // Status frames: onEvent (sync) throttles + forwards details over a
        // channel; a consumer sends them in order before the final reply.
        let (status_tx, mut status_rx) = mpsc::unbounded_channel::<String>();
        let on_event = {
            let now = self.inner.now.clone();
            let throttle = self.inner.config.status_throttle_ms;
            let mut last_status_at: i64 = i64::MIN;
            let status_tx = status_tx.clone();
            Box::new(move |semantic: &mappers::HarnessSemanticEvent| {
                let detail = match status_detail(&semantic.event) {
                    Some(detail) => detail,
                    None => return,
                };
                let current = now();
                if current.saturating_sub(last_status_at) < throttle {
                    return;
                }
                last_status_at = current;
                let _ = status_tx.send(detail);
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

        let options = RunTaskOptions {
            provider,
            prompt: frame.text.clone(),
            cwd: self.inner.config.workspace.clone(),
            env: self.inner.config.env.clone(),
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
            on_event: Some(on_event),
            on_stdin: Some(on_stdin),
        };

        // Consume status details in order while the task runs.
        let status_consumer = {
            let this = self.clone();
            let from = from.clone();
            let task_id = frame.task_id.clone();
            let correlation = correlation.clone();
            tokio::spawn(async move {
                while let Some(detail) = status_rx.recv().await {
                    this.reply(
                        &from,
                        TaskFrameKind::Status,
                        &task_id,
                        &detail,
                        correlation.as_deref(),
                        Some(provider),
                    )
                    .await;
                }
            })
        };

        let result = (self.inner.run_task)(options).await;
        // The task future is dropped here, dropping its on_event (and its status
        // sender); the consumer then drains and ends.
        let _ = status_consumer.await;

        match result {
            Ok(run) => {
                self.reply_with_usage(
                    &from,
                    TaskFrameKind::Reply,
                    &frame.task_id,
                    &run.reply,
                    correlation.as_deref(),
                    Some(provider),
                    run.usage,
                )
                .await;
                self.log(&format!("task {} ✓ ({} events)", frame.task_id, run.events));
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
    pub(super) async fn handle_plain_text(&self, from: String, text: String) {
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
            on_event: None,
            on_stdin: None,
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

    /// Choose a provider: the requested one if offered, else the default, else
    /// the first offered.
    fn select_provider(&self, requested: Option<HarnessProvider>) -> Option<HarnessProvider> {
        let providers = &self.inner.config.providers;
        match requested {
            Some(requested) => providers.contains(&requested).then_some(requested),
            None => {
                if providers.contains(&self.inner.config.default_provider) {
                    Some(self.inner.config.default_provider)
                } else {
                    providers.first().copied()
                }
            }
        }
    }
}
