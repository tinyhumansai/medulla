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

/// The pty ends we keep open for the lifetime of the child.
///
/// Dropped as a unit when the session is reaped: the master and the writer are
/// two file descriptors, and a process that opens sessions for a living runs out
/// of those long before it runs out of anything else.
pub(crate) struct SessionIo {
    /// The PTY master — the resize handle.
    pub(super) master: Box<dyn MasterPty + Send>,
    /// A writer onto the master, kept open for keystrokes and injection.
    pub(super) writer: Box<dyn std::io::Write + Send>,
}

/// The fields that are strings, and therefore cannot be atomics.
///
/// Both are written at most a handful of times per session and read only when a
/// row is built, so one small mutex for the pair costs nothing and keeps the hot
/// fields lock-free.
#[derive(Default)]
pub(crate) struct ColdFields {
    /// The harness session id — minted for claude, read back for codex.
    pub(super) session_id: Option<String>,
    /// Why the session failed, when it did.
    pub(super) last_error: Option<String>,
}

/// The parts of a session that never change after it is opened.
pub(crate) struct SessionMeta {
    /// The manager's stable local id (`w_…`).
    pub(crate) id: String,
    /// The list label — the peer's id.
    pub(crate) label: String,
    /// Which harness is running.
    pub(crate) provider: HarnessProvider,
    /// The working directory the child runs in.
    pub(crate) cwd: String,
    /// Epoch ms when the session started.
    pub(crate) started_at: i64,
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
    /// The string-valued fields, which cannot be atomics.
    pub(super) cold: Mutex<ColdFields>,
    /// The terminal emulator holding this session's screen + scrollback.
    pub(super) screen: Mutex<vt100::Parser>,
    /// The pty ends, until the session is reaped.
    pub(super) io: Mutex<Option<SessionIo>>,
    /// The child handle, for signalling and reaping. `None` once reaped.
    pub(super) child: Mutex<Option<Box<dyn Child + Send + Sync>>>,
}
