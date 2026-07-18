//! The headless `medulla daemon`: offer this machine's local coding-agent CLIs
//! (Claude Code / Codex / OpenCode) as an addressable tiny.place agent over
//! Signal end-to-end encrypted DMs, speaking both plain-text prompts and the
//! `medulla-tinyplace/1` task protocol an orchestrator delegates with.
//!
//! Layout:
//! - [`mappers`] — JSONL transcript → semantic-event line mappers.
//! - [`providers`] — provider detection + one-shot headless task execution.
//! - [`capabilities`] — the on-demand capability probe.
//! - [`transport`] — encrypted Signal DM send/receive + pre-key publishing.
//! - this module — [`DaemonRuntime`], the provider-agnostic task state machine,
//!   plus the CLI entry ([`run_daemon`]) that wires the SDK transport in.
//!
//! The interactive PTY wrapper/bridge (node-pty equivalent), the machine bus, the
//! terminal-envelope writer, and the opencode SSE server are intentionally out of
//! scope here — the interactive wrapper lands separately.

pub mod capabilities;
pub mod mappers;
pub mod providers;
pub mod transport;

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::{mpsc, Mutex as TokioMutex, Notify, Semaphore};

use crate::tinyplace_support::{
    encode_task_frame, AgentCapabilities, EncodeFrameInput, HarnessEvent, HarnessEventKind,
    HarnessProvider, TaskFrame, TaskFrameKind,
};
use tinyplace::auth::timestamp;

use capabilities::{probe_capabilities, ProbeOptions};
use providers::{Abort, RunTaskFn, RunTaskOptions};

const DEFAULT_STATUS_THROTTLE_MS: i64 = 4_000;
const DEFAULT_MAX_PENDING: usize = 16;
const DEFAULT_CAPABILITY_TIMEOUT_MS: u64 = 60_000;

/// A lock-serialized encrypted send: `(to, body) -> ()`. Errors are handled by
/// the transport (logged), so the runtime never observes a send failure.
pub type SendFn =
    Arc<dyn Fn(String, String) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// A clock, in epoch ms (injectable for tests).
pub type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;

/// A line sink for daemon diagnostics.
pub type LogFn = Arc<dyn Fn(&str) + Send + Sync>;

/// Non-callback daemon configuration.
#[derive(Clone)]
pub struct DaemonConfig {
    pub providers: Vec<HarnessProvider>,
    pub default_provider: HarnessProvider,
    pub workspace: String,
    pub env: HashMap<String, String>,
    pub task_timeout_ms: u64,
    pub capability_timeout_ms: Option<u64>,
    pub concurrency: usize,
    pub status_throttle_ms: i64,
    pub max_pending: usize,
    pub model: Option<String>,
    pub agent: Option<String>,
    pub extra_args: Vec<String>,
    pub skip_permissions: bool,
}

struct RunningTask {
    provider: HarnessProvider,
    correlation_id: Option<String>,
    stdin: Option<mpsc::UnboundedSender<String>>,
    pending_input: Vec<String>,
}

struct Inner {
    config: DaemonConfig,
    run_task: RunTaskFn,
    send: SendFn,
    now: NowFn,
    log: Option<LogFn>,
    running: StdMutex<HashMap<String, RunningTask>>,
    controllers: StdMutex<HashMap<u64, Abort>>,
    next_controller_id: AtomicU64,
    admitted: AtomicUsize,
    slots: Semaphore,
    inflight_count: AtomicUsize,
    inflight_idle: Notify,
    capabilities: TokioMutex<Option<AgentCapabilities>>,
}

/// The provider-agnostic daemon task state machine. Cheap to clone (an `Arc`),
/// so it can be handed to spawned dispatches.
#[derive(Clone)]
pub struct DaemonRuntime {
    inner: Arc<Inner>,
}

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
    /// caller; the work runs on a spawned task tracked by [`idle`].
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

    fn log(&self, line: &str) {
        if let Some(log) = &self.inner.log {
            log(line);
        }
    }

    fn task_key(from: &str, task_id: &str) -> String {
        format!("{from} {task_id}")
    }

    fn register_controller(&self, abort: Abort) -> u64 {
        let id = self.inner.next_controller_id.fetch_add(1, Ordering::SeqCst);
        self.inner.controllers.lock().unwrap().insert(id, abort);
        id
    }

    fn unregister_controller(&self, id: u64) {
        self.inner.controllers.lock().unwrap().remove(&id);
    }

    async fn handle_frame(&self, from: String, frame: TaskFrame) {
        match frame.kind {
            TaskFrameKind::Task => self.handle_task(from, frame).await,
            TaskFrameKind::Input => self.handle_input(from, frame).await,
            TaskFrameKind::Capabilities => self.handle_capabilities(from, frame).await,
            // status/reply/error/ack/capabilities_result are responses; ignore.
            _ => {}
        }
    }

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

    async fn handle_input(&self, from: String, frame: TaskFrame) {
        let key = Self::task_key(&from, &frame.task_id);
        let (matched, harness) = {
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
                        (false, self.inner.config.default_provider)
                    } else {
                        let provider = task.provider;
                        match &task.stdin {
                            Some(stdin) => {
                                let _ = stdin.send(frame.text.clone());
                            }
                            None => task.pending_input.push(frame.text.clone()),
                        }
                        (true, provider)
                    }
                }
                None => (false, self.inner.config.default_provider),
            }
        };
        let text = if matched {
            "input received"
        } else {
            "no matching running task for input"
        };
        self.reply(
            &from,
            TaskFrameKind::Ack,
            &frame.task_id,
            text,
            frame.correlation_id.as_deref(),
            Some(harness),
        )
        .await;
    }

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
            model: self.inner.config.model.clone(),
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
                self.reply(
                    &from,
                    TaskFrameKind::Reply,
                    &frame.task_id,
                    &run.reply,
                    correlation.as_deref(),
                    Some(provider),
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

    async fn handle_plain_text(&self, from: String, text: String) {
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

    #[allow(clippy::too_many_arguments)]
    async fn reply(
        &self,
        to: &str,
        kind: TaskFrameKind,
        task_id: &str,
        text: &str,
        correlation: Option<&str>,
        harness: Option<HarnessProvider>,
    ) {
        let body = encode_task_frame(EncodeFrameInput {
            kind,
            task_id: task_id.to_string(),
            text: text.to_string(),
            ts: timestamp(),
            correlation_id: correlation.map(str::to_string),
            harness,
            provider: None,
        });
        self.send_raw(to, &body).await;
    }

    async fn send_raw(&self, to: &str, body: &str) {
        (self.inner.send)(to.to_string(), body.to_string()).await;
    }
}

/// Derive a short status string from a semantic event (or none). Ported from the
/// TS `statusDetail`.
pub fn status_detail(event: &HarnessEvent) -> Option<String> {
    match event.decoded() {
        HarnessEventKind::ToolCall(payload) => Some(cap(
            &format!("running {}: {}", payload.tool_name, payload.display),
            200,
        )),
        HarnessEventKind::ToolResult(payload) => Some(
            if payload.is_error {
                "tool failed"
            } else {
                "tool completed"
            }
            .to_string(),
        ),
        HarnessEventKind::AgentThinking(_) => Some("thinking".to_string()),
        HarnessEventKind::AgentMessage(_) => Some("writing response".to_string()),
        HarnessEventKind::Status(payload) => {
            let detail = if payload.detail.is_empty() {
                payload.state
            } else {
                payload.detail
            };
            (!detail.is_empty()).then_some(detail)
        }
        HarnessEventKind::Error(payload) => Some(cap(&format!("error: {}", payload.message), 200)),
        _ => None,
    }
}

fn cap(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value.to_string()
    } else {
        value.chars().take(max_chars).collect()
    }
}

fn system_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ─── CLI entry ────────────────────────────────────────────────────────────────

pub use entry::run_daemon;

mod entry {
    use super::*;
    use std::io::IsTerminal;
    use std::path::PathBuf;

    use crate::tinyplace_support::{
        config_path, decode_task_frame, load_or_create_identity, resolve_endpoint,
        spawn_contact_auto_accepter, spawn_presence_heartbeat,
    };
    use tinyplace::api::directory::DirectoryApi;
    use tinyplace::api::registry::RegisterRequest;
    use tinyplace::types::AgentCard;
    use tinyplace::{LocalSigner, Signer, TinyPlaceClient, TinyPlaceClientOptions};

    use super::capabilities::read_git_facts;
    use super::providers::{detect_providers, provider_bin, run_provider_task, DAEMON_PROVIDERS};
    use super::transport::SignalTransport;

    const BOOL_FLAGS: &[&str] = &[
        "dangerously-skip-permissions",
        "once",
        "no-onboard",
        "reonboard",
    ];
    const DEFAULT_CONCURRENCY: usize = 2;
    const DEFAULT_TASK_TIMEOUT_MS: u64 = 600_000;
    const DEFAULT_POLL_MS: u64 = 2_000;

    #[derive(Default)]
    struct Flags {
        values: HashMap<String, Vec<String>>,
        bools: HashSet<String>,
    }

    impl Flags {
        fn parse(args: &[String]) -> Result<Self, String> {
            let mut flags = Flags::default();
            let mut index = 0;
            while index < args.len() {
                let token = &args[index];
                let name = token
                    .strip_prefix("--")
                    .ok_or_else(|| format!("unexpected argument: {token}"))?;
                if BOOL_FLAGS.contains(&name) {
                    flags.bools.insert(name.to_string());
                    index += 1;
                } else {
                    let value = args
                        .get(index + 1)
                        .cloned()
                        .ok_or_else(|| format!("--{name} needs a value"))?;
                    flags
                        .values
                        .entry(name.to_string())
                        .or_default()
                        .push(value);
                    index += 2;
                }
            }
            Ok(flags)
        }

        fn string(&self, name: &str) -> Option<String> {
            self.values.get(name).and_then(|v| v.last().cloned())
        }

        fn list(&self, name: &str) -> Option<Vec<String>> {
            self.values.get(name).map(|values| {
                values
                    .iter()
                    .flat_map(|value| value.split(','))
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
        }

        fn number(&self, name: &str) -> Result<Option<u64>, String> {
            match self.string(name) {
                Some(raw) => raw
                    .parse::<u64>()
                    .map(Some)
                    .map_err(|_| format!("--{name} must be a non-negative integer (got {raw})")),
                None => Ok(None),
            }
        }

        fn positive(&self, name: &str, fallback: u64) -> Result<u64, String> {
            match self.number(name)? {
                Some(0) => Err(format!("--{name} must be a positive integer (got 0)")),
                Some(value) => Ok(value),
                None => Ok(fallback),
            }
        }

        fn is_set(&self, name: &str) -> bool {
            self.bools.contains(name)
        }
    }

    fn parse_provider(value: &str) -> Result<HarnessProvider, String> {
        HarnessProvider::from_wire(value).ok_or_else(|| {
            format!(
                "unknown provider \"{value}\" (expected: {})",
                DAEMON_PROVIDERS
                    .iter()
                    .map(|p| p.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
    }

    /// Run `medulla daemon` until a shutdown signal. `args` are the tokens after
    /// the `daemon` subcommand.
    pub async fn run_daemon(args: &[String]) -> anyhow::Result<()> {
        let flags = Flags::parse(args).map_err(|e| anyhow::anyhow!(e))?;
        let env: HashMap<String, String> = std::env::vars().collect();
        let log = |line: &str| eprintln!("medulla daemon: {line}");

        // Provider detection.
        let only = match flags.list("providers") {
            Some(raw) => Some(
                raw.iter()
                    .map(|entry| parse_provider(entry))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| anyhow::anyhow!(e))?,
            ),
            None => None,
        };
        let providers = detect_providers(&env, only.as_deref(), None);
        if providers.is_empty() {
            let wanted = only
                .as_deref()
                .unwrap_or(&DAEMON_PROVIDERS)
                .iter()
                .map(|p| format!("{} ({})", p.as_str(), provider_bin(*p, &env)))
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::bail!(
                "no coding-agent CLI found on PATH — looked for: {wanted}. Install one or pass --providers."
            );
        }

        let default_provider = match flags.string("default-provider") {
            Some(requested) => {
                let provider = parse_provider(&requested).map_err(|e| anyhow::anyhow!(e))?;
                if !providers.contains(&provider) {
                    anyhow::bail!(
                        "--default-provider \"{requested}\" is not available; detected: {}",
                        providers
                            .iter()
                            .map(|p| p.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
                provider
            }
            None => providers[0],
        };

        let workspace = flags
            .string("workspace")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let workspace = std::fs::canonicalize(&workspace)
            .unwrap_or(workspace)
            .to_string_lossy()
            .into_owned();
        let concurrency = flags
            .positive("concurrency", DEFAULT_CONCURRENCY as u64)
            .map_err(|e| anyhow::anyhow!(e))? as usize;
        let task_timeout_ms = flags
            .positive("task-timeout-ms", DEFAULT_TASK_TIMEOUT_MS)
            .map_err(|e| anyhow::anyhow!(e))?;
        let poll_ms = flags
            .positive("poll-ms", DEFAULT_POLL_MS)
            .map_err(|e| anyhow::anyhow!(e))?;
        let max_pending = flags
            .positive("max-pending", DEFAULT_MAX_PENDING as u64)
            .map_err(|e| anyhow::anyhow!(e))? as usize;
        let status_throttle_ms = flags
            .number("status-throttle-ms")
            .map_err(|e| anyhow::anyhow!(e))?
            .map(|v| v as i64)
            .unwrap_or(DEFAULT_STATUS_THROTTLE_MS);
        let model = flags.string("model");
        let opencode_agent = flags.string("opencode-agent");
        let skip_permissions = flags.is_set("dangerously-skip-permissions");
        let handle = flags.string("handle");
        let extra_skills = flags.list("skills").unwrap_or_default();
        let once = flags.is_set("once");
        let reonboard = flags.is_set("reonboard");

        // First-run worker registration (naming + owner setup). On a TTY this
        // walks the operator through onboarding; headless it auto-registers with
        // defaults + an env owner so the daemon stays scriptable. Aborting the
        // interactive flow (q / Ctrl-C) exits cleanly without serving.
        let is_tty = std::io::stdout().is_terminal();
        let worker_profile =
            match crate::onboarding::ensure_registered(&env, is_tty, reonboard).await? {
                Some(reg) => reg.profile,
                None => {
                    log("onboarding aborted; not starting daemon");
                    return Ok(());
                }
            };
        // The profile's name is the daemon's advertised label unless --name overrides it.
        let display_name = flags.string("name").or_else(|| {
            let name = worker_profile.name.trim();
            (!name.is_empty()).then(|| name.to_string())
        });

        // Identity + client.
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let config_file = config_path(&env, &home);
        let (signer, config) = load_or_create_identity(&config_file, &env)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let base_url = resolve_endpoint(&env, &config);
        let signer = Arc::new(signer);
        let client = TinyPlaceClient::new(TinyPlaceClientOptions {
            base_url: base_url.clone(),
            signer: Some(signer.clone() as Arc<dyn Signer>),
            ..Default::default()
        });
        let identity_dir = config_file
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".tinyplace"));

        let transport = SignalTransport::new(client.clone(), &signer, &identity_dir);
        let agent_id = transport.agent_id().to_string();

        // Onboard (publish keys, register handle, upsert directory card) unless
        // suppressed. Key publishing is what lets peers open an encrypted channel.
        if !flags.is_set("no-onboard") {
            let git = read_git_facts(&workspace).await;
            let bio = format!(
                "Headless coding-agent daemon serving {} over tiny.place.{} cwd:{workspace}",
                providers
                    .iter()
                    .map(|p| p.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                git.project
                    .as_ref()
                    .map(|p| format!(" project:{p}"))
                    .unwrap_or_default(),
            );
            let mut skills: Vec<String> = std::iter::once("coding-agent".to_string())
                .chain(providers.iter().map(|p| p.as_str().to_string()))
                .chain(extra_skills.iter().cloned())
                .collect();
            dedupe(&mut skills);
            onboard(
                &transport,
                &signer,
                &client.directory,
                &agent_id,
                handle.as_deref(),
                display_name.as_deref(),
                &bio,
                &skills,
                &client,
                log,
            )
            .await;
            log(&format!(
                "onboarded {agent_id} (skills: {})",
                skills.join(", ")
            ));
        }

        // Runtime + transport-backed send.
        let send: SendFn = {
            let transport = transport.clone();
            Arc::new(move |to: String, body: String| {
                let transport = transport.clone();
                Box::pin(async move {
                    if let Err(err) = transport.send(&to, &body).await {
                        eprintln!("medulla daemon: send to {to} failed: {err}");
                    }
                })
            })
        };
        let config = DaemonConfig {
            providers: providers.clone(),
            default_provider,
            workspace: workspace.clone(),
            env: env.clone(),
            task_timeout_ms,
            capability_timeout_ms: None,
            concurrency,
            status_throttle_ms,
            max_pending,
            model,
            agent: opencode_agent,
            extra_args: Vec::new(),
            skip_permissions,
        };
        let run_task: RunTaskFn =
            Arc::new(|options: RunTaskOptions| Box::pin(run_provider_task(options)));
        let runtime = DaemonRuntime::new(config, run_task, send)
            .with_log(Arc::new(|line: &str| eprintln!("medulla daemon: {line}")));

        // Contact auto-accept + presence run unlocked (pure REST, no ratchet).
        let accepter = spawn_contact_auto_accepter(
            client.clone(),
            std::time::Duration::from_millis(poll_ms),
            |_agent_id: &str| true,
        );
        let presence =
            spawn_presence_heartbeat(client.clone(), std::time::Duration::from_millis(poll_ms));

        if once {
            // Probe hook: accept pending contacts, drain the inbox once, wait for
            // every started task to settle, then exit.
            drain_once(&transport, &runtime).await;
            runtime.idle().await;
            accepter.abort();
            presence.abort();
            log("--once complete");
            return Ok(());
        }

        log(&format!(
            "serving providers [{}] as {agent_id} on {base_url} (workspace: {workspace})",
            providers
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));

        // Serve loop: poll → decode → dispatch, until a signal.
        let poll = tokio::time::Duration::from_millis(poll_ms);
        let mut sigterm = signal_stream()?;
        loop {
            tokio::select! {
                _ = &mut sigterm => {
                    log("received shutdown signal, shutting down");
                    break;
                }
                _ = tokio::time::sleep(poll) => {
                    for message in transport.drain_inbox(50).await {
                        let frame = decode_task_frame(&message.text);
                        runtime.handle_message(message.from, message.text, frame);
                    }
                }
            }
        }

        runtime.shutdown();
        accepter.abort();
        presence.abort();
        Ok(())
    }

    async fn drain_once(transport: &SignalTransport, runtime: &DaemonRuntime) {
        for message in transport.drain_inbox(50).await {
            let frame = crate::tinyplace_support::decode_task_frame(&message.text);
            runtime.handle_message(message.from, message.text, frame);
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn onboard(
        transport: &SignalTransport,
        signer: &LocalSigner,
        directory: &DirectoryApi,
        agent_id: &str,
        handle: Option<&str>,
        display_name: Option<&str>,
        bio: &str,
        skills: &[String],
        client: &TinyPlaceClient,
        log: impl Fn(&str),
    ) {
        // Publish Signal pre-keys (required for peers to message us).
        match transport.publish_keys(signer).await {
            Ok(()) => log("published Signal pre-keys"),
            Err(err) => log(&format!("pre-key publish failed: {err}")),
        }

        // Claim the handle (best-effort; needs funds).
        if let Some(handle) = handle {
            let result = client
                .registry
                .register(RegisterRequest {
                    username: handle.to_string(),
                    crypto_id: agent_id.to_string(),
                    ..Default::default()
                })
                .await;
            match result {
                Ok(_) => log(&format!("registered handle {handle}")),
                Err(err) => log(&format!("handle registration skipped: {err}")),
            }
        }

        // Upsert the directory card (best-effort). AgentCard has no Default, so
        // build it from JSON with only the fields we set (the rest default).
        let name = display_name
            .map(str::to_string)
            .or_else(|| handle.map(str::to_string))
            .unwrap_or_else(|| "coding-agent daemon".to_string());
        let card: AgentCard = serde_json::from_value(serde_json::json!({
            "agentId": agent_id,
            "name": name,
            "description": bio,
            "username": handle,
            "cryptoId": agent_id,
            "skills": skills,
        }))
        .expect("AgentCard JSON is well-formed");
        match directory.upsert_agent(agent_id, &card).await {
            Ok(_) => log("upserted directory card"),
            Err(err) => log(&format!("directory upsert skipped: {err}")),
        }
    }

    fn dedupe(values: &mut Vec<String>) {
        let mut seen = HashSet::new();
        values.retain(|value| seen.insert(value.clone()));
    }

    /// A future that resolves on SIGINT/SIGTERM (Unix) or Ctrl-C (elsewhere).
    fn signal_stream() -> anyhow::Result<Pin<Box<dyn Future<Output = ()> + Send>>> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigint = signal(SignalKind::interrupt())?;
            let mut sigterm = signal(SignalKind::terminate())?;
            Ok(Box::pin(async move {
                tokio::select! {
                    _ = sigint.recv() => {}
                    _ = sigterm.recv() => {}
                }
            }))
        }
        #[cfg(not(unix))]
        {
            Ok(Box::pin(async move {
                let _ = tokio::signal::ctrl_c().await;
            }))
        }
    }

    #[cfg(test)]
    mod flag_tests {
        use super::*;

        fn args(list: &[&str]) -> Vec<String> {
            list.iter().map(|s| s.to_string()).collect()
        }

        #[test]
        fn parses_values_bools_and_lists() {
            let flags = Flags::parse(&args(&[
                "--workspace",
                "/tmp/x",
                "--providers",
                "claude,codex",
                "--providers",
                "opencode",
                "--once",
                "--dangerously-skip-permissions",
            ]))
            .unwrap();
            assert_eq!(flags.string("workspace").as_deref(), Some("/tmp/x"));
            assert!(flags.is_set("once"));
            assert!(flags.is_set("dangerously-skip-permissions"));
            assert!(!flags.is_set("no-onboard"));
            // Repeated + comma-joined lists flatten and trim.
            assert_eq!(
                flags.list("providers").unwrap(),
                vec!["claude", "codex", "opencode"]
            );
            // A later value wins for scalar lookups.
            let dup = Flags::parse(&args(&["--model", "a", "--model", "b"])).unwrap();
            assert_eq!(dup.string("model").as_deref(), Some("b"));
        }

        #[test]
        fn rejects_unknown_and_valueless_flags() {
            assert!(Flags::parse(&args(&["positional"])).is_err());
            match Flags::parse(&args(&["--model"])) {
                Err(err) => assert!(err.contains("needs a value"), "got: {err}"),
                Ok(_) => panic!("missing value should error"),
            }
        }

        #[test]
        fn number_and_positive_validation() {
            let flags = Flags::parse(&args(&[
                "--concurrency",
                "3",
                "--zero",
                "0",
                "--bad",
                "nope",
            ]))
            .unwrap();
            assert_eq!(flags.number("concurrency").unwrap(), Some(3));
            assert_eq!(flags.number("missing").unwrap(), None);
            assert!(flags.number("bad").is_err());
            assert_eq!(flags.positive("concurrency", 2).unwrap(), 3);
            assert_eq!(flags.positive("missing", 7).unwrap(), 7);
            assert!(flags.positive("zero", 2).is_err());
        }

        #[test]
        fn parse_provider_maps_wire_names() {
            assert_eq!(parse_provider("claude").unwrap(), HarnessProvider::Claude);
            assert_eq!(parse_provider("codex").unwrap(), HarnessProvider::Codex);
            let err = parse_provider("bogus").unwrap_err();
            assert!(err.contains("unknown provider"), "got: {err}");
        }

        #[test]
        fn dedupe_preserves_first_occurrence_order() {
            let mut values = args(&["a", "b", "a", "c", "b"]);
            dedupe(&mut values);
            assert_eq!(values, vec!["a", "b", "c"]);
        }
    }
}

#[cfg(test)]
mod tests;
