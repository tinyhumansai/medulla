//! Data model for PTY-backed harness sessions: how one is launched, what the
//! operator watches about it, and the handle the UI holds.

use medulla::protocol::HarnessProvider;
pub use medulla::sessions::SessionOrigin;

use super::attention::HarnessAttention;

/// Default geometry for a freshly opened session, before the UI reports the real
/// pane size. Wide enough that a harness's first full-screen paint is not
/// mangled by an 80-column assumption it then has to reflow out of.
pub const DEFAULT_COLS: u16 = 120;
/// Default row count for a freshly opened session.
pub const DEFAULT_ROWS: u16 = 30;

/// How many lines of scrollback the emulator retains per session.
pub const SCROLLBACK: usize = 2_000;

/// Where a PTY session is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyState {
    /// The child is running.
    Running,
    /// The child exited; the last screen is retained so the operator can read it.
    Exited {
        /// The child's exit status, when it reported one.
        code: Option<i32>,
    },
    /// The child could not be started, or its PTY died.
    Failed,
}

impl PtyState {
    /// The display string.
    pub fn as_str(self) -> &'static str {
        match self {
            PtyState::Running => "running",
            PtyState::Exited { .. } => "exited",
            PtyState::Failed => "failed",
        }
    }

    /// A single-width glyph for the session list.
    pub fn glyph(self) -> char {
        match self {
            PtyState::Running => '●',
            PtyState::Exited { code: Some(0) } | PtyState::Exited { code: None } => '✓',
            PtyState::Exited { .. } => '✕',
            PtyState::Failed => '✕',
        }
    }

    /// Whether the child is still alive.
    pub fn is_running(self) -> bool {
        matches!(self, PtyState::Running)
    }
}

/// Who is allowed to drive an agent session right now.
///
/// Keyboard focus ([`HarnessFocus`](crate::ui::harness_pane::HarnessFocus)) says
/// where *keystrokes* go; this says who holds *authority*. They are not the same
/// thing, and conflating them is what let the orchestrator paste a task prompt
/// into a composer the operator was already typing in — a session serves one turn
/// at a time, so two writers produce one confidently wrong answer rather than an
/// error.
///
/// Nor is it [`SessionOrigin`], which is who *started* the session. Control
/// moves — `ctrl-g` takes a session and handing it back returns it — and origin
/// never does. A task-spawned session an operator is holding is
/// `origin: orchestrator, control: user`; both halves are true at once, and only
/// this one gates dispatch.
///
/// This is the single gate on dispatch: [`claim_idle`](super::PtyManager::claim_idle)
/// only ever returns an orchestrator-held session. An "unmanaged" session is not a
/// separate kind of thing — it is one that was born [`User`](Self::User)-held and
/// has not been handed over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionControl {
    /// The orchestrator may dispatch task frames into this session.
    #[default]
    Orchestrator,
    /// The operator holds it. Dispatch skips it entirely until it is handed back.
    User,
}

impl SessionControl {
    /// The display string, from the operator's point of view.
    ///
    /// "you" rather than "user" because it is rendered next to a session the
    /// person reading it is looking at.
    pub fn as_str(self) -> &'static str {
        match self {
            SessionControl::Orchestrator => "orchestrator",
            SessionControl::User => "you",
        }
    }

    /// Whether the orchestrator may dispatch into a session in this state.
    pub fn is_orchestrator(self) -> bool {
        matches!(self, SessionControl::Orchestrator)
    }

    /// The other side of the handover, for a toggle.
    pub fn toggled(self) -> Self {
        match self {
            SessionControl::Orchestrator => SessionControl::User,
            SessionControl::User => SessionControl::Orchestrator,
        }
    }
}

/// How to launch one harness session.
#[derive(Debug, Clone)]
pub struct LaunchSpec {
    /// Which coding-agent CLI to run.
    pub provider: HarnessProvider,
    /// The custom preset this session was launched from, when it was one.
    ///
    /// A preset is a *different agent* running the same CLI — its own model,
    /// endpoint and environment — so its id, not the base CLI's wire name, is
    /// what a declaration records for it. Carrying it here is what lets the rail
    /// file the session under the agent that declared it; without it a
    /// preset-backed session was compared as `claude` against a declaration
    /// saying `deepseek` and listed as belonging to no agent at all.
    ///
    /// `None` for a native CLI entry, whose id *is* the provider's wire name.
    pub preset: Option<String>,
    /// The resolved binary name or path.
    pub bin: String,
    /// Working directory for the child.
    pub cwd: String,
    /// Environment for the child.
    pub env: std::collections::HashMap<String, String>,
    /// Extra argv appended after the provider's interactive base args.
    pub extra_args: Vec<String>,
    /// Whether to launch with the provider's permission-bypass flag.
    ///
    /// A watched session is still unattended: nobody is sitting in the pane to
    /// answer "allow this command?", so a task that stops on one has hung.
    pub skip_permissions: bool,
    /// A label for the session list — the peer's id.
    pub label: String,
    /// An operator- or peer-configured model override, when set.
    ///
    /// Threaded onto the interactive argv the same way the headless path adds
    /// it to its one-shot invocation: without this, a `[host].model` or a
    /// task's own model hint silently reached the harness's own default
    /// instead, changing which model actually ran.
    pub model: Option<String>,
    /// A session id to hand the harness, when it accepts one.
    ///
    /// Claude does (`--session-id`), and the transcript is then written under
    /// that name — so it can be found by identity rather than by guessing which
    /// file is newest. Codex has no such flag; its rollout records its own id on
    /// line one, which the tailer reads back instead.
    pub session_id: Option<String>,
    /// Who holds the session the moment it opens.
    ///
    /// A task frame opens an [`Orchestrator`](SessionControl::Orchestrator)
    /// session; an operator spawning one from the TUI opens a
    /// [`User`](SessionControl::User) one, which is what makes it unmanaged.
    pub control: SessionControl,
    /// Who is starting this session — see [`SessionOrigin`].
    ///
    /// Display and labelling only — never gate behaviour on it. Control is the
    /// gate; this is what lets a session be shown under its agent as "started
    /// by you" rather than the less useful "orchestrator-spawned, currently
    /// yours". It is fixed here for the session's whole life.
    pub origin: SessionOrigin,
    /// The display name the operator gave this session as they spun it up.
    ///
    /// `None` for a dispatched session, which is labelled from its task
    /// instead, and `None` today for an operator-started one as well — the
    /// picker has no name prompt yet. The field is the seam that lets one
    /// appear without touching this layer again.
    pub name: Option<String>,
    /// The key this session's MCP fleet grant was minted under, when one was.
    ///
    /// Carried so the grant can be handed back when the harness exits: a
    /// capability that outlives the session it was minted for is a token
    /// nothing revokes and a leaked subprocess could still redeem.
    pub mcp_grant_session: Option<String>,
}

/// The operator-facing projection of one session, for the list pane.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRow {
    /// The manager's stable local id (`w_…`).
    pub id: String,
    /// The list label.
    pub label: String,
    /// Which harness is running.
    pub provider: HarnessProvider,
    /// The custom preset it was launched from — see [`LaunchSpec::preset`].
    ///
    /// Together with `provider` this gives the session's *harness id*, which is
    /// the vocabulary a declaration is written in: [`harness_id`](Self::harness_id).
    pub preset: Option<String>,
    /// Where the child is in its life.
    pub state: PtyState,
    /// The working directory the child runs in.
    pub cwd: String,
    /// Git branch resolved from the working directory when the session opened.
    ///
    /// `None` means the directory is not in a repository or has a detached
    /// `HEAD`.
    pub branch: Option<String>,
    /// Repository root captured immediately before launch.
    pub launch_root: Option<String>,
    /// Commit checked out in the working directory immediately before launch.
    ///
    /// This immutable snapshot anchors the Changes view even if the harness
    /// creates commits while it runs. `None` means the directory was outside a
    /// repository or the repository did not have a first commit yet.
    pub launch_commit: Option<String>,
    /// Filesystem identity of the Git directory captured at launch.
    pub launch_checkout_identity: Option<String>,
    /// The harness session id, once known — minted for claude, read back from
    /// the rollout for codex. This is what pins the transcript tailer.
    pub session_id: Option<String>,
    /// Human-facing thread name advertised by the harness through the terminal
    /// title, including names changed interactively with `/rename`.
    pub thread_name: Option<String>,
    /// Epoch ms when the session started.
    pub started_at: i64,
    /// Epoch ms of the last output byte — the liveness signal the list shows.
    pub last_output_at: i64,
    /// Why it failed, when it did.
    pub last_error: Option<String>,
    /// Whether a turn is running in this session right now.
    ///
    /// A harness serves one turn at a time: two prompts typed into one composer
    /// are answered as one conversation, and both tails settle on the same
    /// completion. So a busy session is not reusable, however idle its pty
    /// looks.
    pub busy: bool,
    /// Who holds this session right now — see [`SessionControl`].
    pub control: SessionControl,
    /// Who started it — see [`SessionOrigin`]. Immutable, and independent of
    /// [`SessionRow::control`]: taking a dispatched session does not make it
    /// yours to have started.
    pub origin: SessionOrigin,
    /// The name the operator gave it when they started it, if they gave one.
    ///
    /// `None` for a dispatched session; the UI labels those from their task.
    pub name: Option<String>,
    /// What this harness is waiting on the operator for, if anything.
    ///
    /// The one piece of session state nothing else on this row can express. A
    /// harness stopped on a permission prompt is running, not busy in any way
    /// the orchestrator knows about, and producing no output — so `state`,
    /// `busy`, and `last_output_at` all read exactly as they do for a harness
    /// thinking hard, and the operator watching a different pane never learns
    /// it stopped. Recomputed from the screen as it paints (see
    /// [`attention`](super::attention)), and what makes the row blink.
    ///
    /// `None` is the ordinary state: working, idle at a composer, or exited.
    pub attention: Option<HarnessAttention>,
}

impl SessionRow {
    /// Milliseconds since the harness last wrote anything.
    pub fn idle_ms(&self, now: i64) -> i64 {
        now.saturating_sub(self.last_output_at).max(0)
    }

    /// The stable harness id this session runs as — a preset's own id, else the
    /// provider's wire name.
    ///
    /// The same id a picker choice reports and a declaration records, which is
    /// what makes matching a session to its agent a comparison of like with
    /// like rather than of a preset against the CLI underneath it.
    pub fn harness_id(&self) -> &str {
        self.preset
            .as_deref()
            .filter(|preset| !preset.trim().is_empty())
            .unwrap_or_else(|| self.provider.as_str())
    }
}
