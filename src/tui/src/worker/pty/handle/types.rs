//! Data types for one live PTY session.
#[allow(unused_imports)]
use super::*;

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
/// All three are written at most a handful of times per session and read only
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
    /// Why the session failed, when it did.
    pub(super) last_error: Option<String>,
}

/// The parts of a session that never change after it is opened.
pub(crate) struct SessionMeta {
    /// The manager's stable local id (`w_…`).
    pub(crate) id: String,
    /// Which harness is running.
    pub(crate) provider: HarnessProvider,
    /// The working directory the child runs in.
    pub(crate) cwd: String,
    /// Git branch resolved from the working directory when the session opened.
    ///
    /// `None` means the directory is not in a repository or has a detached
    /// `HEAD`.
    pub(crate) branch: Option<String>,
    /// Epoch ms when the session started.
    pub(crate) started_at: i64,
    /// Whether an operator asked for this session rather than a task frame.
    ///
    /// Display only — never gate behaviour on it. Control is the gate; this is
    /// what lets the rail say "unmanaged", and what marks a session whose
    /// synthetic `you:` label is still up for adoption.
    pub(crate) user_spawned: bool,
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
    /// [`HarnessControl`](super::super::types::HarnessControl) as one bit, for
    /// the same reason `busy` is: `claim_idle` tests it for every session on
    /// every dispatch, and it is the gate that stops a task prompt landing in a
    /// composer a person is typing in.
    pub(super) operator_held: AtomicBool,
    /// How many bytes sit in [`SessionIo::writes`] still unwritten.
    ///
    /// The budget a caller is admitted against, so a child that never drains its
    /// stdin cannot make the queue grow without limit. Reserved before queueing
    /// and released as each write leaves, so two concurrent writers cannot both
    /// see room and both take it.
    ///
    /// An `Arc` because the writer thread releases the reservation as it drains,
    /// and it outlives the `io` the queue itself lives in.
    pub(super) queued_bytes: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    /// The string-valued fields, which cannot be atomics.
    pub(super) cold: Mutex<ColdFields>,
    /// The terminal emulator holding this session's screen + scrollback.
    pub(super) screen: Mutex<vt100::Parser>,
    /// The pty ends, until the session is reaped.
    pub(super) io: Mutex<Option<SessionIo>>,
    /// The child handle, for signalling and reaping. `None` once reaped.
    pub(super) child: Mutex<Option<Box<dyn Child + Send + Sync>>>,
}
