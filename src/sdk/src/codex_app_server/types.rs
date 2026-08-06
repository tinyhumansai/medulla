//! Data model for the pooled app-server client: what identifies a shareable
//! process, how one is spawned, what a thread and a turn are parameterised by,
//! and what a finished turn reports.
//!
//! Behaviour lives in the sibling `connection` and `pool` modules; this file is
//! shapes and their trivial impls only.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fmt;

use serde::Serialize;

use crate::protocol::TokenUsage;

/// How this client identifies itself in `initialize`.
///
/// Codex stamps it into the user agent it sends upstream, so it is how an
/// app-server run is attributable to Medulla at all.
pub const CLIENT_NAME: &str = "medulla";

/// Environment variables that change *who a Codex process is* rather than what
/// one task asks of it.
///
/// Every one of these is part of the pool key. A process authenticates once and
/// holds that identity for its whole life, so two tasks that disagree on any of
/// them cannot share one — they must get separate processes or one of them runs
/// as the wrong account against the wrong endpoint.
///
/// Deliberately a fixed list rather than "the whole environment": hashing every
/// variable would key on `PWD` and `_` and defeat pooling entirely, which is the
/// one thing this module exists to provide.
const IDENTITY_ENV: [&str; 8] = [
    "CODEX_HOME",
    "CODEX_API_KEY",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "OPENAI_ORG_ID",
    "OPENAI_PROJECT",
    "CODEX_MEDULLA_PROFILE",
    "http_proxy",
];

/// The identity of a shareable app-server process.
///
/// Two [`AppServerSpec`]s with equal keys may share a connection; unequal keys
/// get separate processes. Constructed by [`AppServerKey::from_spec`] so the
/// rule for what counts lives in exactly one place.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AppServerKey {
    /// The resolved `codex` binary this process runs.
    bin: String,
    /// Extra argv the operator pinned, which may select a profile or config.
    args: Vec<String>,
    /// The identity-bearing environment, sorted for a stable key.
    identity: Vec<(String, String)>,
}

impl AppServerKey {
    /// Derive the sharing key from a spawn spec.
    pub fn from_spec(spec: &AppServerSpec) -> Self {
        let identity = IDENTITY_ENV
            .iter()
            .filter_map(|name| {
                spec.env
                    .get(*name)
                    .map(|value| ((*name).to_string(), value.clone()))
            })
            .collect::<BTreeMap<_, _>>()
            .into_iter()
            .collect();
        Self {
            bin: spec.bin.clone(),
            args: spec.args.clone(),
            identity,
        }
    }

    /// A short, stable label for logs and diagnostics.
    ///
    /// Names the binary and the Codex home, which is what an operator needs to
    /// tell two pooled processes apart. Never includes a credential: the
    /// identity list holds API keys, and a log line is not a place for one.
    pub fn label(&self) -> String {
        let home = self
            .identity
            .iter()
            .find(|(name, _)| name == "CODEX_HOME")
            .map(|(_, value)| value.as_str())
            .unwrap_or("(default home)");
        format!("{} [{home}]", self.bin)
    }
}

/// Everything needed to spawn one app-server process.
#[derive(Debug, Clone)]
pub struct AppServerSpec {
    /// The `codex` binary to run, already resolved through the bin-override
    /// contract.
    pub bin: String,
    /// Extra argv inserted before the `app-server` subcommand.
    pub args: Vec<String>,
    /// The full environment the child runs with; the parent environment is
    /// cleared, matching the CLI spawn seam.
    pub env: HashMap<String, String>,
}

/// Codex's filesystem/network sandbox for a thread.
///
/// Mirrors the protocol's `SandboxMode`, restated here so callers do not depend
/// on the wire spelling and so the enum can carry Medulla's own doc comments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SandboxMode {
    /// No writes, no network.
    #[serde(rename = "read-only")]
    ReadOnly,
    /// Writes confined to the working directory; network off.
    #[serde(rename = "workspace-write")]
    WorkspaceWrite,
    /// Unrestricted. What `skip_permissions` selects — see
    /// [`ThreadOptions::from_permissions`] for why a delegated task needs it.
    #[serde(rename = "danger-full-access")]
    DangerFullAccess,
}

/// When Codex stops to ask the client for approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ApprovalPolicy {
    /// Ask before anything the sandbox would refuse.
    #[serde(rename = "on-request")]
    OnRequest,
    /// Never ask.
    #[serde(rename = "never")]
    Never,
}

/// Per-thread parameters — the things two threads in one process may legitimately
/// disagree about.
#[derive(Debug, Clone)]
pub struct ThreadOptions {
    /// Working directory for the thread.
    pub cwd: String,
    /// Model override, or `None` for whatever Codex is configured with.
    pub model: Option<String>,
    /// Filesystem/network sandbox.
    pub sandbox: SandboxMode,
    /// Approval policy.
    pub approval_policy: ApprovalPolicy,
    /// A previously captured thread id to resume instead of starting fresh.
    pub resume_thread_id: Option<String>,
}

impl ThreadOptions {
    /// Build thread options for a delegated task from the operator's single
    /// permissions consent.
    ///
    /// The mapping mirrors the CLI seam exactly. `skip_permissions` on gets
    /// `danger-full-access` + `never`, because Codex's default
    /// `workspace-write` cannot serve a delegated task: the git directory of a
    /// linked worktree sits *outside* the worktree, so `git commit` cannot write
    /// its refs, and with the network off `git push` and `gh` fail outright.
    ///
    /// With consent withheld the thread runs `workspace-write` + `on-request`,
    /// and the connection declines the approvals that follow — the same refusal
    /// the CLI seam produces, rather than a prompt nobody is watching.
    pub fn from_permissions(cwd: String, model: Option<String>, skip_permissions: bool) -> Self {
        let (sandbox, approval_policy) = if skip_permissions {
            (SandboxMode::DangerFullAccess, ApprovalPolicy::Never)
        } else {
            (SandboxMode::WorkspaceWrite, ApprovalPolicy::OnRequest)
        };
        Self {
            cwd,
            model,
            sandbox,
            approval_policy,
            resume_thread_id: None,
        }
    }
}

/// How a turn ended, as Codex reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnStatus {
    /// The turn ran to completion.
    Completed,
    /// The turn was interrupted, by us or by the operator.
    Interrupted,
    /// The turn failed; the message is on [`TurnOutcome::error`].
    Failed,
}

impl TurnStatus {
    /// Parse the protocol's `TurnStatus` string. `inProgress` is not a terminal
    /// status and so has no variant here; an unknown value is treated as failed
    /// rather than silently reported as success.
    pub fn from_wire(value: &str) -> Self {
        match value {
            "completed" => TurnStatus::Completed,
            "interrupted" => TurnStatus::Interrupted,
            _ => TurnStatus::Failed,
        }
    }
}

/// What one finished turn produced.
#[derive(Debug, Clone, Default)]
pub struct TurnOutcome {
    /// The thread this turn ran on, for a later resume.
    pub thread_id: String,
    /// Concatenated assistant messages, in order.
    pub reply: String,
    /// Token counts Codex reported for the thread, when it reported any.
    pub usage: Option<TokenUsage>,
    /// The failure message, when the turn failed.
    pub error: Option<String>,
    /// Count of thread items observed, for the same "did anything happen"
    /// accounting the CLI seam does with semantic events.
    pub items: usize,
}

/// Everything that can go wrong talking to an app-server.
#[derive(Debug)]
pub enum AppServerError {
    /// The process could not be spawned, or died.
    Process(String),
    /// The transport broke — a closed pipe, an unreadable line.
    Transport(String),
    /// The server answered a request with a JSON-RPC error.
    Rpc {
        /// The method that failed.
        method: String,
        /// The server's message.
        message: String,
    },
    /// A response arrived in a shape this client does not understand.
    Protocol(String),
    /// The turn produced no activity within the idle budget.
    Idle {
        /// The budget that elapsed, in milliseconds.
        timeout_ms: u64,
    },
    /// The task was aborted cooperatively.
    Aborted,
}

impl fmt::Display for AppServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppServerError::Process(message) => write!(f, "codex app-server process: {message}"),
            AppServerError::Transport(message) => {
                write!(f, "codex app-server transport: {message}")
            }
            AppServerError::Rpc { method, message } => {
                write!(f, "codex app-server `{method}` failed: {message}")
            }
            AppServerError::Protocol(message) => write!(f, "codex app-server protocol: {message}"),
            AppServerError::Idle { timeout_ms } => {
                write!(
                    f,
                    "codex app-server turn idle for {timeout_ms}ms (no updates)"
                )
            }
            AppServerError::Aborted => write!(f, "codex app-server turn aborted"),
        }
    }
}

impl std::error::Error for AppServerError {}
