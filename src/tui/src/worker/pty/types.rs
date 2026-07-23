//! Data model for PTY-backed harness sessions: how one is launched, what the
//! operator watches about it, and the handle the UI holds.

use std::sync::{Arc, Mutex};

use medulla::tinyplace::HarnessProvider;
use portable_pty::{Child, MasterPty};

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
    /// A session id to hand the harness, when it accepts one.
    ///
    /// Claude does (`--session-id`), and the transcript is then written under
    /// that name — so it can be found by identity rather than by guessing which
    /// file is newest. Codex has no such flag; its rollout records its own id on
    /// line one, which the tailer reads back instead.
    pub session_id: Option<String>,
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
    /// A writer onto the master, kept open for input injection.
    pub(super) writer: Box<dyn std::io::Write + Send>,
    /// The child handle, for signalling and reaping.
    ///
    /// `Option` so the reaper can take it out and block on `wait()` *without*
    /// holding the manager's lock — see [`PtyManager`](super::manager::PtyManager)'s
    /// `mark_finished`. `None` means the child has been reaped.
    pub(super) child: Option<Box<dyn Child + Send + Sync>>,
}
