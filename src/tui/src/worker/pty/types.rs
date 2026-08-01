//! Data model for PTY-backed harness sessions: how one is launched, what the
//! operator watches about it, and the handle the UI holds.

use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

use medulla::tinyplace::HarnessProvider;
use portable_pty::{Child, MasterPty};

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

/// Who is allowed to drive a harness session right now.
///
/// Keyboard focus ([`HarnessFocus`](crate::ui::harness_pane::HarnessFocus)) says
/// where *keystrokes* go; this says who holds *authority*. They are not the same
/// thing, and conflating them is what let the orchestrator paste a task prompt
/// into a composer the operator was already typing in — a harness serves one turn
/// at a time, so two writers produce one confidently wrong answer rather than an
/// error.
///
/// This is the single gate on dispatch: [`claim_idle`](super::PtyManager::claim_idle)
/// only ever returns an orchestrator-held session. An "unmanaged" harness is not a
/// separate kind of thing — it is one that was born [`User`](Self::User)-held and
/// has not been handed over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HarnessControl {
    /// The orchestrator may dispatch task frames into this session.
    #[default]
    Orchestrator,
    /// The operator holds it. Dispatch skips it entirely until it is handed back.
    User,
}

impl HarnessControl {
    /// The display string, from the operator's point of view.
    ///
    /// "you" rather than "user" because it is rendered next to a harness the
    /// person reading it is looking at.
    pub fn as_str(self) -> &'static str {
        match self {
            HarnessControl::Orchestrator => "orchestrator",
            HarnessControl::User => "you",
        }
    }

    /// Whether the orchestrator may dispatch into a session in this state.
    pub fn is_orchestrator(self) -> bool {
        matches!(self, HarnessControl::Orchestrator)
    }

    /// The other side of the handover, for a toggle.
    pub fn toggled(self) -> Self {
        match self {
            HarnessControl::Orchestrator => HarnessControl::User,
            HarnessControl::User => HarnessControl::Orchestrator,
        }
    }
}

/// How to launch one harness session.
#[derive(Debug, Clone)]
pub struct LaunchSpec {
    /// Which coding-agent CLI to run.
    pub provider: HarnessProvider,
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
    /// A task frame opens an [`Orchestrator`](HarnessControl::Orchestrator)
    /// session; an operator spawning one from the TUI opens a
    /// [`User`](HarnessControl::User) one, which is what makes it unmanaged.
    pub control: HarnessControl,
    /// Whether an operator asked for this session rather than a task frame.
    ///
    /// Display only — never gate behaviour on it. Control is the gate; this
    /// exists so the rail can say "unmanaged" instead of the less useful
    /// "orchestrator-spawned, currently yours".
    pub user_spawned: bool,
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
    /// Where the child is in its life.
    pub state: PtyState,
    /// The working directory the child runs in.
    pub cwd: String,
    /// Git branch resolved from the working directory when the session opened.
    ///
    /// `None` means the directory is not in a repository or has a detached
    /// `HEAD`.
    pub branch: Option<String>,
    /// The harness session id, once known — minted for claude, read back from
    /// the rollout for codex. This is what pins the transcript tailer.
    pub session_id: Option<String>,
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
    /// Who holds this session right now — see [`HarnessControl`].
    pub control: HarnessControl,
    /// Whether an operator spawned it rather than a task frame. Display only.
    pub user_spawned: bool,
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
}

/// One live PTY-backed harness session.
///
/// The emulator screen is behind its own mutex so the reader task can feed it
/// while the render thread reads it, without either blocking on the child.
pub(super) struct PtySession {
    /// The operator-facing projection.
    pub(super) row: SessionRow,
    /// The terminal emulator holding this session's screen + scrollback.
    pub(super) screen: Arc<Mutex<vt100::Parser>>,
    /// The PTY master — the write side (keystrokes in) and the resize handle.
    pub(super) master: Box<dyn MasterPty + Send>,
    /// Queue onto this session's writer thread, which owns the master's write
    /// half.
    ///
    /// The writer itself is deliberately **not** held here. A pty write parks in
    /// the kernel for as long as the child leaves its stdin undrained — a harness
    /// still loading, or sitting on a startup dialog, does exactly that — and
    /// every path that reaches a session, including the render pass on every
    /// frame, goes through the manager's single lock. Holding the writer here
    /// meant [`PtyManager::write`](super::manager::PtyManager::write) performed
    /// that blocking write with the lock held, so one unread paste froze the
    /// whole TUI.
    ///
    /// The channel itself is unbounded, because a bounded one blocks its sender
    /// once full — the very failure being removed. The bound lives in
    /// [`queued_bytes`](Self::queued_bytes) instead, where it can be enforced by
    /// refusing rather than by waiting.
    pub(super) writes: std::sync::mpsc::Sender<Vec<u8>>,
    /// How many bytes sit in [`writes`](Self::writes) still unwritten.
    ///
    /// The budget a caller is admitted against, so a child that never drains its
    /// stdin cannot make the queue grow without limit. Reserved before queueing
    /// and released as each write leaves, so two concurrent writers cannot both
    /// see room and both take it.
    ///
    /// Counted in bytes rather than messages: one write is an arbitrarily long
    /// paste, so a message count would bound nothing that matters.
    pub(super) queued_bytes: Arc<AtomicUsize>,
    /// The emulator's audible-bell count as of the last attention refresh.
    ///
    /// The bell is an *event*, not a screen state: by the time anything reads
    /// the emulator the ring is over, and only the counter says it happened. So
    /// what was already seen is remembered here, and any increase since is a
    /// harness asking for the operator.
    pub(super) seen_bells: usize,
    /// Revision of the session's attention state.
    ///
    /// Classification reads the screen without holding the sessions lock. A
    /// release increments this revision so a classifier that started before
    /// the release cannot restore a completion cue after it was consumed.
    pub(super) attention_generation: u64,
    /// Whether the next newly observed bell belongs to the just-completed turn.
    ///
    /// Transcript settlement can release a session shortly before the CLI emits
    /// its completion bell. The next poll consumes that late chime. Claiming a
    /// new turn or attaching to acknowledge the pane clears the suppression, so
    /// a later turn's bell remains meaningful.
    pub(super) suppress_next_bell: bool,
    /// Epoch ms of the last attention refresh.
    ///
    /// The reader thread wakes on every read — a full-screen repaint is dozens
    /// of them — and reclassifying the whole screen that often would burn a core
    /// re-reading the same frame. Attention is a human-facing signal, so it is
    /// recomputed on a human timescale (see `ATTENTION_INTERVAL_MS`).
    pub(super) attention_checked_at: i64,
    /// The child handle, for signalling and reaping.
    ///
    /// `Option` so the reaper can take it out and block on `wait()` *without*
    /// holding the manager's lock — see [`PtyManager`](super::manager::PtyManager)'s
    /// `mark_finished`. `None` means the child has been reaped.
    pub(super) child: Option<Box<dyn Child + Send + Sync>>,
}
