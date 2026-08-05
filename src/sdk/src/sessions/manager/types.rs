//! Data model for [`SessionManager`]: its configuration,
//! the request to open a session, the per-session entry it stores, and the
//! transcript line the UI renders.

use std::collections::HashMap;

use crate::daemon::providers::Abort;
use crate::protocol::HarnessProvider;

use super::super::interactive::InteractiveSession;
use super::super::types::{
    SessionClass, SessionDriver, SessionOrigin, SessionPolicy, SessionRecord,
};

/// How the manager spawns and routes sessions.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Absolute working directory sessions run in.
    pub workspace: String,
    /// Environment passed to spawned harness processes.
    pub env: HashMap<String, String>,
    /// Provider used when a session names none.
    pub default_provider: HarnessProvider,
    /// The operator's class-routing pin.
    pub policy: SessionPolicy,
    /// Default model hint passed to harnesses.
    pub model: Option<String>,
    /// opencode agent selector, when applicable.
    pub agent: Option<String>,
    /// Extra CLI args forwarded to every harness invocation.
    pub extra_args: Vec<String>,
    /// Whether to pass the harness's skip-permissions flag.
    pub skip_permissions: bool,
    /// Per-turn idle watchdog budget, in ms.
    pub turn_timeout_ms: u64,
    /// Optional custom OpenAI-compatible router, layered into each *one-shot*
    /// turn's spawn environment so headless turns route like daemon tasks.
    /// Interactive (PTY) sessions are not routed yet — `InteractiveSpec` carries
    /// no router. `None` means routing is off everywhere.
    pub router: Option<crate::config::RouterConfig>,
    /// Whether commits made by this session's harness are attributed to Medulla
    /// — the resolved `attribution.commit` config value (on by default; see
    /// [`crate::config::AttributionConfig`]).
    pub attribution: bool,
    /// Lifecycle hooks installed into every harness launched here — the
    /// resolved `[[hooks]]` config section (see [`crate::harness_hooks`]).
    pub hooks: crate::harness_hooks::HooksConfig,
}

impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig {
            workspace: ".".to_string(),
            env: HashMap::new(),
            default_provider: HarnessProvider::Claude,
            policy: SessionPolicy::Auto,
            model: None,
            agent: None,
            extra_args: Vec::new(),
            skip_permissions: false,
            turn_timeout_ms: 300_000,
            router: None,
            attribution: true,
            hooks: crate::harness_hooks::HooksConfig::default(),
        }
    }
}

/// A request to register a new session.
#[derive(Debug, Clone)]
pub struct OpenSession {
    /// The conversation anchor — a peer cryptoId, or an operator-chosen label.
    pub conversation: String,
    /// Which harness serves it; `None` uses the configured default.
    pub provider: Option<HarnessProvider>,
    /// The lifetime class; `None` routes from the stimulus and policy.
    pub class: Option<SessionClass>,
    /// Where this session's turns come from.
    pub driver: SessionDriver,
    /// Override the configured workspace for this session.
    pub workspace: Option<String>,
    /// Override the configured model for this session.
    pub model: Option<String>,
    /// Who is starting this session — set by the creation path and never
    /// changed afterwards. See [`SessionOrigin`].
    pub origin: SessionOrigin,
    /// The name the person starting it gave it; `None` for a dispatch.
    pub name: Option<String>,
}

impl OpenSession {
    /// An operator-opened, task-driven session on `conversation`.
    ///
    /// Defaults to [`SessionClass::Unbound`] because an operator opening a
    /// session from the TUI means to talk to it — a bounded session would be
    /// gone before they could type. A person is asking for it, so it is
    /// [`SessionOrigin::User`]-originated; name it with
    /// [`with_name`](Self::with_name).
    pub fn operator(conversation: impl Into<String>) -> Self {
        OpenSession {
            conversation: conversation.into(),
            provider: None,
            class: Some(SessionClass::Unbound),
            driver: SessionDriver::Task,
            workspace: None,
            model: None,
            origin: SessionOrigin::User,
            name: None,
        }
    }

    /// A session opened to serve a dispatched task on `conversation`.
    ///
    /// The counterpart of [`operator`](Self::operator): same shape, opposite
    /// provenance, and never named — the UI labels it from its task.
    pub fn orchestrator(conversation: impl Into<String>) -> Self {
        OpenSession {
            origin: SessionOrigin::Orchestrator,
            ..OpenSession::operator(conversation)
        }
    }

    /// Give the session the display name a person typed.
    ///
    /// Naming does not change provenance: an orchestrator-originated session
    /// that someone later labels is still `origin: orchestrator`.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the harness provider.
    pub fn with_provider(mut self, provider: HarnessProvider) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Set the lifetime class.
    pub fn with_class(mut self, class: SessionClass) -> Self {
        self.class = Some(class);
        self
    }

    /// Set the turn-source driver.
    pub fn with_driver(mut self, driver: SessionDriver) -> Self {
        self.driver = driver;
        self
    }
}

/// Who produced a transcript line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptRole {
    /// A prompt submitted into the session.
    User,
    /// The harness's answer text.
    Agent,
    /// A tool invocation.
    Tool,
    /// A lifecycle/progress note from the manager or an observed envelope.
    Status,
    /// A failure.
    Error,
}

impl TranscriptRole {
    /// The display string.
    pub fn as_str(self) -> &'static str {
        match self {
            TranscriptRole::User => "user",
            TranscriptRole::Agent => "agent",
            TranscriptRole::Tool => "tool",
            TranscriptRole::Status => "status",
            TranscriptRole::Error => "error",
        }
    }

    /// The theme color name for this role, for the Sessions detail pane.
    pub fn color(self) -> &'static str {
        match self {
            TranscriptRole::User => "cyan",
            TranscriptRole::Agent => "green",
            TranscriptRole::Tool => "magenta",
            TranscriptRole::Status => "blue",
            TranscriptRole::Error => "red",
        }
    }
}

/// One line of a session's conversation, as rendered in the detail pane.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptLine {
    /// Epoch ms when the line was recorded.
    pub at: i64,
    /// Who produced it.
    pub role: TranscriptRole,
    /// The text.
    pub text: String,
}

/// The manager's per-session state: the rendered record plus the live process
/// and abort handle behind it.
pub(in crate::sessions) struct SessionEntry {
    /// The operator-facing projection.
    pub(in crate::sessions) record: SessionRecord,
    /// The live interactive process, once opened. `None` for a one-shot session
    /// or an unbound one whose process has not started yet.
    pub(in crate::sessions) live: Option<std::sync::Arc<InteractiveSession>>,
    /// Aborts the in-flight turn. Replaced after each turn so a stale abort
    /// cannot cancel the next one.
    pub(in crate::sessions) abort: Abort,
    /// Bounded transcript ring for the detail pane.
    pub(in crate::sessions) transcript: Vec<TranscriptLine>,
    /// This session's model override, when it has one.
    pub(in crate::sessions) model: Option<String>,
}

/// How many transcript lines to retain per session before dropping the oldest.
pub(super) const TRANSCRIPT_CAP: usize = 500;

#[allow(unused_imports)]
use super::*;
/// A clock in epoch ms (injectable for tests).
pub type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;
pub(in super::super) struct Inner {
    pub(in super::super) config: SessionConfig,
    pub(in super::super) registry: SessionRegistry,
    pub(in super::super) run_task: RunTaskFn,
    pub(in super::super) now: NowFn,
    pub(in super::super) sessions: Mutex<Vec<SessionEntry>>,
    pub(in super::super) next_id: AtomicU64,
    pub(in super::super) changed: broadcast::Sender<()>,
}
/// Spins up and manages coding-agent sessions in both lifetime classes.
///
/// Cheap to clone (an `Arc`), so the daemon, the hub, and the TUI can share one.
#[derive(Clone)]
pub struct SessionManager {
    pub(in super::super) inner: Arc<Inner>,
}
