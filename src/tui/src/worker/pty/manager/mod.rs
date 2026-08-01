//! [`PtyManager`] — the registry of live harness PTYs.
//!
//! One session is: a real `claude`/`codex`/`opencode` child on a pseudo-terminal,
//! a reader thread draining the master into a [`vt100::Parser`], and the write
//! half kept open so keystrokes and injected peer prompts can reach the child.
//! All of that belongs to [`SessionHandle`](super::handle::SessionHandle); what
//! is here is only the collection of them.
//!
//! That division is the design. This used to be a `Mutex<Vec<PtySession>>` where
//! every operation — down to a reader thread stamping a timestamp after each
//! `read()` — took one process-wide lock and scanned the vector under it. It
//! worked at one or two harnesses and fell over at ten, because the data being
//! guarded was per-session and independent: the lock was doing nothing except
//! serialising unrelated work. Now the registry is read-mostly, every accessor
//! clones out the one `Arc` it needs and drops the lock immediately, and the
//! hot per-session fields are atomics on the handle.
//!
//! Split so no file exceeds the repo's 500-line ceiling: [`open`] launches a
//! harness on a fresh pty and drains it, [`session`] is the bookkeeping every
//! other caller reads, and [`screen`] is the emulator surface the UI renders.
//!
//! Both halves of the master run on **blocking threads**, not tokio tasks:
//! `portable-pty` offers only synchronous `Read`/`Write`, and parking either on
//! the async runtime would occupy a worker forever. The reader feeds the shared
//! emulator and exits when the master closes; the writer drains a per-session
//! queue.
//!
//! The writer's thread earns its keep beyond that. A pty write blocks until the
//! child drains its stdin, and a harness that is still loading or sitting on a
//! startup dialog does not — so writing inline, under the one lock every reader
//! here took, froze the whole TUI on a single unread paste. Every method on this
//! type therefore holds the registry for a lookup and an `Arc` clone and nothing
//! more: see [`handle`](Self::handle), which is that seam, and
//! [`write`](Self::write), [`forget`](Self::forget), and `SessionHandle::reap`,
//! which each act on the session with the registry already released. [`rows`](Self::rows)
//! and [`running_count`](Self::running_count) are the deliberate exceptions —
//! they read atomics only, never block, and run every frame.

use std::sync::atomic::AtomicU64;
use std::sync::{Arc, RwLock};

use super::handle::SessionHandle;
use super::sync::{read, write};

/// Read buffer for the PTY master.
///
/// Sized to swallow a whole burst rather than a screenful. The emulator's lock
/// is taken once per read, so the buffer sets how often a flooding child makes
/// the render pass wait: at 8 KB a harness streaming at pipe speed reacquired
/// the lock hundreds of times a second, in a loop tight enough that a waiting
/// snapshot could be starved for tens of milliseconds — a visible stutter, and
/// one that only appeared once the old global lock stopped accidentally pacing
/// the reader. Amortising the wakeup is the same trick NAPI plays on a network
/// device: take more per turn so you take fewer turns.
const BUF_LEN: usize = 65536;

/// How many times to retry a failed `openpty` before giving up.
///
/// Pty allocation is a shared, finite system resource, so it can fail
/// transiently when several processes open sessions at once — on a busy build
/// machine, or simply when a peer's tasks arrive in a burst. Mirrors the
/// ETXTBSY spawn retry the headless executor already carries for the same class
/// of momentary failure.
///
/// Only *transient* failures are retried; see `open::is_transient`. Descriptor
/// exhaustion is a capacity signal, not a race, and retrying it merely burns the
/// whole budget before failing with the same error.
const OPENPTY_ATTEMPTS: u32 = 20;
/// Base pause between `openpty` retries, scaled by attempt number and capped by
/// [`OPENPTY_RETRY_CAP`].
///
/// Backed off rather than fixed: a burst of sessions opening together would
/// otherwise retry in lockstep, colliding on every attempt for the same reason
/// they collided on the first.
const OPENPTY_RETRY_PAUSE: std::time::Duration = std::time::Duration::from_millis(25);
/// Ceiling on the backoff, so twenty attempts stay inside a sensible budget.
const OPENPTY_RETRY_CAP: std::time::Duration = std::time::Duration::from_millis(200);

/// Kill every surviving child when the last handle goes away.
///
/// A pty and its harness outlive the manager otherwise, because neither
/// `portable-pty`'s `Child` nor the master fd terminates the process on drop.
/// Relying on an explicit `shutdown()` makes that a discipline the panic path
/// does not follow — and each leaked session holds a pty device, which the OS
/// has a fixed supply of.
impl Drop for Inner {
    fn drop(&mut self) {
        // Take the handles out and release the registry before waiting on
        // anything: `reap` blocks, and nothing about shutdown should hold a lock
        // across it.
        let sessions = std::mem::take(&mut *write(&self.sessions));
        for session in &sessions {
            session.kill();
        }
        let now = (self.now)();
        for session in &sessions {
            // Reap it: a killed child left unwaited is a zombie holding its slot
            // until this process exits.
            session.reap(now);
        }
    }
}

impl Default for PtyManager {
    fn default() -> Self {
        PtyManager::new()
    }
}

impl PtyManager {
    /// Build an empty manager on the system clock.
    pub fn new() -> Self {
        PtyManager::with_now(Arc::new(medulla::clock::now_millis))
    }

    /// Override the clock (tests).
    pub fn with_now(now: NowFn) -> Self {
        PtyManager {
            inner: Arc::new(Inner {
                sessions: RwLock::new(Vec::new()),
                next_id: AtomicU64::new(1),
                now,
            }),
        }
    }

    fn now(&self) -> i64 {
        (self.inner.now)()
    }

    /// The handle for `id`, if there is one.
    ///
    /// The registry lock is held only for the lookup and the `Arc` clone; every
    /// caller then works against the session alone. This is the seam that stops
    /// a blocking write, a slow `wait()`, or a large snapshot on one session
    /// from stalling all the others.
    pub(super) fn handle(&self, id: &str) -> Option<Arc<SessionHandle>> {
        read(&self.inner.sessions)
            .iter()
            .find(|session| session.id() == id)
            .cloned()
    }

    /// Every handle, in open order.
    pub(super) fn handles(&self) -> Vec<Arc<SessionHandle>> {
        read(&self.inner.sessions).clone()
    }
}

mod open;
mod screen;
mod session;

pub use screen::{ScreenCell, ScreenSnapshot};

mod types;
use types::Inner;
pub use types::NowFn;
pub use types::PtyManager;
