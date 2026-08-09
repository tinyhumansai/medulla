//! Data types for one live PTY session.
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, AtomicUsize};
use std::sync::{Arc, Mutex};

use medulla::protocol::HarnessProvider;
use portable_pty::{Child, MasterPty};

use super::super::attention::HarnessAttention;
use super::super::types::SessionOrigin;

/// [`SessionHandle::state`] discriminants. Kept as a `u8` so the whole liveness
/// question is one relaxed atomic load rather than a lock.
pub(super) const STATE_RUNNING: u8 = 0;
/// The child exited.
pub(super) const STATE_EXITED: u8 = 1;
/// The child could not be started, or its pty died.
pub(super) const STATE_FAILED: u8 = 2;

/// Sentinel for "exited, but with no status we could read".
pub(super) const NO_EXIT_CODE: i64 = i64::MIN;

/// How many unwritten bytes one session's write queue may hold.
///
/// The queue cannot block its sender — that is the freeze this design exists to
/// avoid — so it is bounded by refusal instead: a write that would exceed this
/// is rejected synchronously and the caller is told the harness is not reading.
///
/// Bounded by *bytes* rather than by message count, because one write is an
/// arbitrarily long paste and a count would bound nothing. Generous enough that
/// no real prompt or burst of keystrokes can reach it — a delegated instruction
/// is kilobytes, and `inject_prompt` retries a paste three times at most — so in
/// practice this is a backstop against a caller that does not yet exist rather
/// than a limit anything legitimate will meet.
pub const MAX_QUEUED_WRITE_BYTES: usize = 1024 * 1024;

/// Give `bytes` back to a session's write budget, never going below zero.
///
/// Saturating rather than a bare `fetch_sub`, because the two callers can race
/// in one specific way. The writer thread ends by storing 0 — a queue nothing
/// will drain must not keep occupying the budget — while a concurrent
/// [`SessionHandle::write`](super::SessionHandle::write) may already have
/// reserved its bytes and not yet sent them. If that send then fails because the
/// writer has just gone, the write path releases a reservation the store has
/// already cleared, and on an unsigned counter `fetch_sub` wraps to about
/// `usize::MAX` — which reads as permanently over-quota and refuses every later
/// write to that session.
///
/// Today that is unreachable in effect: the write path checks liveness before it
/// consults the budget, and a session whose writer has gone is reaped moments
/// later, so those writes are already refused for a different reason. That is
/// two accidents holding it up, either of which a later change could remove, and
/// the counter is the sort of thing that eventually gets surfaced as a metric.
/// Cheaper to make it unrepresentable.
pub(crate) fn release_queued(queued_bytes: &AtomicUsize, bytes: usize) {
    let _ = queued_bytes.fetch_update(
        std::sync::atomic::Ordering::AcqRel,
        std::sync::atomic::Ordering::Acquire,
        |queued| Some(queued.saturating_sub(bytes)),
    );
}

/// The pty ends we keep open for the lifetime of the child.
///
/// Dropped as a unit when the session is reaped: the master and the write queue
/// hold file descriptors between them, and a process that opens sessions for a
/// living runs out of those long before it runs out of anything else. Dropping
/// the queue is also what ends the writer thread, so a reaped session leaves no
/// thread parked on a receive that will never fire.
pub(crate) struct SessionIo {
    /// The PTY master — the resize handle.
    pub(super) master: Box<dyn MasterPty + Send>,
    /// Queue onto this session's writer thread, which owns the master's write
    /// half.
    ///
    /// The writer itself is deliberately **not** held here. A pty write parks in
    /// the kernel for as long as the child leaves its stdin undrained — a harness
    /// still loading, or sitting on a startup dialog, does exactly that — so
    /// writing inline made the caller wait on the child. Splitting the sessions
    /// apart (see [`SessionHandle`]) stopped that freezing *other* sessions;
    /// the queue is what stops it freezing the caller, which on the keystroke
    /// path is the render thread itself.
    ///
    /// The channel is unbounded, because a bounded one blocks its sender once
    /// full — the very failure being removed. The bound lives in
    /// [`SessionHandle::queued_bytes`] instead, where it can be enforced by
    /// refusing rather than by waiting.
    pub(super) writes: std::sync::mpsc::Sender<Vec<u8>>,
}

/// The fields that are strings, and therefore cannot be atomics.
///
/// These are written at most a handful of times per session and read only
/// when a row is built, so one small mutex for the set costs nothing and keeps
/// the hot fields lock-free.
#[derive(Default)]
pub(crate) struct ColdFields {
    /// The list label — the peer's id, or `you:<harness>` for one an operator
    /// started.
    ///
    /// Mutable, and only for one transition: a handed-back operator session
    /// adopts the label of the first real conversation that claims it, so later
    /// reuse obeys the same exact-label rule as any task-spawned session.
    pub(super) label: String,
    /// The harness session id — minted for claude, read back for codex.
    pub(super) session_id: Option<String>,
    /// The display name a person gave this session.
    ///
    /// Mutable, unlike [`SessionMeta::origin`] beside it: a name is a label the
    /// operator owns and may change, where provenance is a fact about how the
    /// session came to exist. `None` until somebody names it.
    pub(super) name: Option<String>,
    /// The thread name discovered from the harness provider own session
    /// state rather than from the terminal stream.
    ///
    /// Codex persists renamed threads in its session index instead of emitting
    /// an OSC window title, so the executor supplies this separately.  When
    /// set, it takes precedence over [[thread_name]] so that ordinary PTY
    /// output (which may carry an empty title) cannot erase the index-backed
    /// name.
    pub(super) index_thread_name: Option<String>,
    /// The non-empty terminal title last advertised by the harness.
    pub(super) thread_name: Option<String>,
    /// Which repository, worktree, and branch the working directory sits in.
    ///
    /// Mutable, and the reason this is not in [`SessionMeta`] beside the
    /// working directory it is read from: a harness that creates a branch, or
    /// an operator who switches one under it, changes the answer while the
    /// session runs, and a launch-time snapshot went on naming the branch the
    /// work started from for as long as the session lived. Refreshed by
    /// `PtyManager::spawn_checkout_poller`; the checkout *identity* that pins
    /// the diff baseline is a different, deliberately immutable fact — see
    /// [`SessionMeta::launch_checkout_identity`].
    pub(super) checkout: medulla::ui::checkout::Checkout,
    /// Why the session failed, when it did.
    pub(super) last_error: Option<String>,
}

/// Mutable state used by the background attention classifier.
///
/// Kept behind its own mutex so sampling one terminal never takes the manager's
/// registry lock or contends with unrelated sessions.
pub(crate) struct AttentionState {
    /// The cue currently shown in the rail.
    pub(crate) cue: Option<HarnessAttention>,
    /// The emulator bell watermark already classified.
    pub(crate) seen_bells: usize,
    /// Revision used to reject a sample raced by release or acknowledgement.
    pub(crate) generation: u64,
    /// Completion chimes promised by settled turns but not observed yet.
    pub(crate) pending_completion_bells: usize,
    /// Deadline before which promised chimes may arrive ahead of session reuse.
    pub(crate) completion_deadline: Option<std::time::Instant>,
    /// Epoch ms of the last classification attempt.
    pub(crate) checked_at: i64,
}

/// The parts of a session that never change after it is opened.
pub(crate) struct SessionMeta {
    /// The manager's stable local id (`w_…`).
    pub(crate) id: String,
    /// Which harness is running.
    pub(crate) provider: HarnessProvider,
    /// The custom preset it was launched from, when it was one — see
    /// [`LaunchSpec::preset`](super::super::types::LaunchSpec::preset).
    ///
    /// Immutable like the rest of the meta: which preset ran is a fact about
    /// this session's birth, and it is half of the harness id the rail matches
    /// a session to its agent by.
    pub(crate) preset: Option<String>,
    /// The working directory the child runs in.
    pub(crate) cwd: String,
    /// Whether the child environment contains a GitHub repository override.
    pub(crate) gh_repo_is_set: bool,
    /// Repository root captured immediately before the harness was spawned.
    pub(crate) launch_root: Option<String>,
    /// Commit checked out immediately before the harness process was spawned.
    pub(crate) launch_commit: Option<String>,
    /// Filesystem identity of the launch checkout's Git directory.
    pub(crate) launch_checkout_identity: Option<String>,
    /// Epoch ms when the session started.
    pub(crate) started_at: i64,
    /// Who started this session — see
    /// [`SessionOrigin`](super::super::types::SessionOrigin).
    ///
    /// It lives in the immutable half of the handle *because* it is immutable:
    /// control is an atomic bit that takeover flips, and origin is a fact about
    /// this session's birth that nothing may rewrite. Display and labelling
    /// only — never gate behaviour on it — but it is what marks a session whose
    /// synthetic `you:` label is still up for adoption.
    pub(crate) origin: SessionOrigin,
    /// The key this session's MCP fleet grant was minted under, when one was.
    ///
    /// Read twice over a session's life, at the two ends of the capability: on
    /// reap, to give it back — or narrow it to reporting, if the session left a
    /// workflow run executing — and again when the operator forgets the row, to
    /// drop the runs that were drawn beneath it.
    pub(crate) mcp_grant_session: Option<String>,
}

/// One live PTY-backed harness session, and everything the manager knows about
/// it.
///
/// The shape is the point. This used to be a plain struct inside a
/// `Mutex<Vec<PtySession>>`, so *every* operation — including a reader thread
/// stamping a timestamp after each `read()` — took one process-wide lock and
/// scanned the vector under it. That is a big kernel lock: the data it guarded
/// was almost entirely per-session and independent, and the contention showed up
/// as the whole TUI stalling whenever several harnesses were busy.
///
/// So the state is split by how it is actually used:
///
/// - [`SessionMeta`] is immutable, so it needs no synchronisation at all;
/// - the hot, single-value fields (liveness, last output, screen generation,
///   whether a turn is running) are **atomics**, so the reader thread's
///   per-read update is a relaxed store with no lock and no lookup;
/// - the genuinely shared, genuinely mutable things — the emulator, the pty
///   ends, the child — get **their own** mutexes, so a slow operation on one
///   session cannot block any other. That is what stops a wedged child, whose
///   input buffer is full and whose `write` is parked in the kernel, from
///   freezing every render frame and every other harness with it.
///
/// The manager holds these behind an `Arc`, hands the `Arc` out, and drops its
/// registry lock immediately — so no caller ever holds the registry while doing
/// work.
pub struct SessionHandle {
    pub(super) meta: SessionMeta,
    /// One of the `STATE_*` discriminants.
    pub(super) state: AtomicU8,
    /// The exit status, or [`NO_EXIT_CODE`].
    pub(super) exit_code: AtomicI64,
    /// Epoch ms of the last output byte — the liveness signal the list shows.
    pub(super) last_output_at: AtomicI64,
    /// Bumped every time the emulator consumes input.
    ///
    /// A change counter, in the spirit of a framebuffer's damage bit: a caller
    /// that remembers the generation it last rendered can skip rebuilding a
    /// snapshot for a screen that has not moved, which is one atomic load
    /// instead of a few thousand allocations.
    pub(super) generation: AtomicU64,
    /// Whether a turn is running in this session right now.
    pub(super) busy: AtomicBool,
    /// Whether the operator, rather than the orchestrator, holds this session.
    ///
    /// [`SessionControl`](super::super::types::SessionControl) as one bit, for
    /// the same reason `busy` is: `claim_idle` tests it for every session on
    /// every dispatch, and it is the gate that stops a task prompt landing in a
    /// composer a person is typing in.
    pub(super) operator_held: AtomicBool,
    /// Whether this session has finished the task it was opened for and is being
    /// kept for the operator to read.
    ///
    /// A lifecycle fact, deliberately *not* a
    /// [`SessionControl`](super::super::types::SessionControl) variant. Control
    /// answers "who may type here", and a retained session has no one typing in
    /// it yet — it is the last screen of finished work, left standing because
    /// closing it is what made a completed task look like it had vanished.
    ///
    /// The distinction is load-bearing rather than cosmetic. Marking these
    /// sessions `User` instead would have been the smaller change and would have
    /// deadlocked dispatch: `checkout_writer` reads any user-held session in a
    /// directory as the writer holding that checkout, so the first task to
    /// finish in a workspace would have queued every task after it until their
    /// budgets expired.
    ///
    /// An atomic beside `busy` and `operator_held` for the same reason those
    /// are: [`try_claim`](super::SessionHandle::try_claim) tests it per session
    /// on every dispatch.
    pub(super) retained: AtomicBool,
    /// How many bytes sit in [`SessionIo::writes`] still unwritten.
    ///
    /// The budget a caller is admitted against, so a child that never drains its
    /// stdin cannot make the queue grow without limit. Reserved before queueing
    /// and released as each write leaves, so two concurrent writers cannot both
    /// see room and both take it.
    ///
    /// An `Arc` because the writer thread releases the reservation as it drains,
    /// and it outlives the `io` the queue itself lives in.
    pub(super) queued_bytes: Arc<AtomicUsize>,
    /// The string-valued fields, which cannot be atomics.
    pub(super) cold: Mutex<ColdFields>,
    /// Human-attention cue and its bell/race bookkeeping.
    pub(crate) attention: Mutex<AttentionState>,
    /// The terminal emulator holding this session's screen + scrollback.
    pub(super) screen: Mutex<vt100::Parser>,
    /// Parser for terminal modes the screen emulator does not expose publicly.
    pub(super) modes: Mutex<TerminalModes>,
    /// The pty ends, until the session is reaped.
    pub(super) io: Mutex<Option<SessionIo>>,
    /// The child handle, for signalling and reaping. `None` once reaped.
    pub(super) child: Mutex<Option<Box<dyn Child + Send + Sync>>>,
}

/// Terminal modes needed by the host but not exposed by [`vt100::Screen`].
pub(super) struct TerminalModes {
    /// Stateful parser, retained so escape sequences split across PTY reads work.
    pub(super) parser: vte::Parser,
    /// Independent OSC 52 capture, retained across reads for the same reason.
    ///
    /// See [`super::osc52`] for why `vte`'s own OSC dispatch is not enough on
    /// its own: its OSC buffer is a fixed 1024 bytes, so a copy larger than
    /// that arrives truncated through [`vte::Perform::osc_dispatch`] alone.
    pub(super) osc52: super::osc52::Osc52Scanner,
    /// Whether xterm alternate-scroll mode (DECSET 1007) is enabled.
    pub(super) alternate_scroll: bool,
    /// The clipboard write the child last asked for (OSC 52), until it is taken.
    ///
    /// A harness copying something — a `y` in its own copy mode, a `tmux
    /// load-buffer -w` run inside the pane, a script echoing the escape — is
    /// asking *its* terminal for the clipboard, and its terminal is us. Nobody
    /// downstream would ever see it otherwise: `vt100` drops OSC 52, and the
    /// child's bytes are parsed into a screen grid rather than replayed to our
    /// own stdout, so the copy would die in this process. Captured here and
    /// [taken](super::SessionHandle::take_clipboard) by the reader thread, which
    /// forwards it on to the operator's terminal.
    ///
    /// Last write wins, which is what a clipboard is: a copy nobody has
    /// collected yet is superseded by the next one, exactly as in a real
    /// terminal.
    pub(super) clipboard: Option<String>,
}

impl Default for TerminalModes {
    fn default() -> Self {
        Self {
            parser: vte::Parser::new(),
            osc52: super::osc52::Osc52Scanner::default(),
            alternate_scroll: false,
            clipboard: None,
        }
    }
}

impl vte::Perform for TerminalModes {
    /// Capture the child's clipboard writes; every other OSC is the screen
    /// emulator's business.
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if let Some(text) = medulla::clipboard::tmux::osc52_from_params(params) {
            self.clipboard = Some(text);
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        ignore: bool,
        action: char,
    ) {
        if !ignore
            && intermediates == [b'?']
            && matches!(action, 'h' | 'l')
            && params.iter().any(|param| param == [1007])
        {
            self.alternate_scroll = action == 'h';
        }
    }
}
