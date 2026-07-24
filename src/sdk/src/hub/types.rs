//! Data types for the tiny.place task-sender hub: a dispatch request, its
//! terminal outcome, and the error a dispatch can fail with.

use std::time::Duration;

use crate::tinyplace::{HarnessProvider, TokenUsage};

/// A line sink for hub diagnostics.
///
/// The hub used to write these straight to stderr, which is fine for
/// `medulla hub` but corrupts the orchestrator TUI: ratatui owns the alternate
/// screen and only repaints the cells it manages, so anything else written to
/// the terminal lands on top and is never cleared. Injecting the sink lets a
/// caller that owns the screen route the lines somewhere it can render them.
pub type HubLog = crate::logging::LineSink;

/// Where roster changes are written so they survive a restart.
///
/// A callback rather than a config-file write inside the handle: the hub should
/// not have to know where an embedding host keeps its settings, and a test needs
/// to observe the roster without touching a disk.
pub type RosterSink = std::sync::Arc<dyn Fn(&[super::HubWorker]) + Send + Sync>;

/// The default sink: stderr, as before, for callers that own the terminal.
pub fn stderr_log() -> HubLog {
    crate::logging::stderr_sink()
}

/// A single task to dispatch to a remote tiny.place worker.
#[derive(Debug, Clone)]
pub struct TaskRequest {
    /// Cycle-scoped task id (echoed on the frame; the worker returns it).
    pub task_id: String,
    /// Correlates every frame in one cycle; `None` uses the literal `"cyc"`.
    pub cycle_id: Option<String>,
    /// The instruction/prompt the worker runs.
    pub instruction: String,
    /// The worker's tiny.place address (base58 cryptoId or `@handle`).
    pub worker_address: String,
    /// Optional harness hint (`claude`/`codex`/`opencode`).
    pub provider: Option<HarnessProvider>,
    /// Optional model hint (the worker maps it to `--model`/`-m`, else its
    /// configured default).
    pub model: Option<String>,
    /// How long to wait for a terminal `reply`/`error` before giving up.
    pub timeout: Duration,
}

/// The terminal result of a dispatched task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskOutcome {
    /// The worker's final reply text.
    pub reply: String,
    /// Token usage the worker reported (zeros when it reported none).
    pub usage: TokenUsage,
    /// The provider that actually ran the task, when the worker reported it.
    pub harness: Option<HarnessProvider>,
}

/// Why a dispatch failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// No terminal frame arrived before the request's `timeout`.
    Timeout,
    /// The worker returned an `error` frame (carrying its message).
    Worker(String),
    /// The send itself failed, or the waiter was dropped (transport-shaped).
    Transport(String),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Timeout => write!(f, "tiny.place task timed out"),
            RunError::Worker(m) => write!(f, "worker error: {m}"),
            RunError::Transport(m) => write!(f, "transport error: {m}"),
        }
    }
}

impl std::error::Error for RunError {}
