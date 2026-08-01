//! [`PtyManager`] — owns every live harness PTY and its terminal emulator.
//!
//! One session is: a real `claude`/`codex`/`opencode` child on a pseudo-terminal,
//! a reader thread draining the master into a [`vt100::Parser`], and the write
//! half kept open so keystrokes and injected peer prompts can reach the child.
//!
//! Split so no file exceeds the repo's 500-line ceiling: [`open`] launches a
//! harness on a fresh pty and drains it, [`session`] is the bookkeeping every
//! other caller reads, [`screen`] is the emulator surface the UI renders, and
//! [`attention`] keeps each row's "this harness wants you" flag current.
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
//! here takes, froze the whole TUI on a single unread paste. Every method on this
//! type is therefore expected to hold the lock for bookkeeping only: see
//! `mark_finished`, which drops it before reaping a child, and `write`, which
//! drops it before queueing bytes.

use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use super::types::PtySession;

/// Read buffer for the PTY master, sized for a full-screen redraw burst.
const BUF_LEN: usize = 8192;

/// How many unwritten bytes one session's write queue may hold.
///
/// The queue cannot block its sender — that is the freeze this module exists to
/// avoid — so it is bounded by refusal instead: a write that would exceed this
/// is rejected synchronously and the caller is told the harness is not reading.
///
/// Bounded by *bytes* rather than by message count, because one write is an
/// arbitrarily long paste and a count would bound nothing. Generous enough that
/// no real prompt or burst of keystrokes can reach it — a delegated instruction
/// is kilobytes, and `inject_prompt` retries a paste three times at most — so in
/// practice this is a backstop against a caller that does not yet exist rather
/// than a limit anything legitimate will meet.
const MAX_QUEUED_WRITE_BYTES: usize = 1024 * 1024;

/// How many times to retry a failed `openpty` before giving up.
///
/// Pty allocation is a shared, finite system resource, so it can fail
/// transiently when several processes open sessions at once — on a busy build
/// machine, or simply when a peer's tasks arrive in a burst. Mirrors the
/// ETXTBSY spawn retry the headless executor already carries for the same class
/// of momentary failure.
const OPENPTY_ATTEMPTS: u32 = 20;
/// Pause between `openpty` retries.
const OPENPTY_RETRY_PAUSE: std::time::Duration = std::time::Duration::from_millis(25);

/// Kill every surviving child when the last handle goes away.
///
/// A pty and its harness outlive the manager otherwise, because neither
/// `portable-pty`'s `Child` nor the master fd terminates the process on drop.
/// Relying on an explicit `shutdown()` makes that a discipline the panic path
/// does not follow — and each leaked session holds a pty device, which the OS
/// has a fixed supply of.
impl Drop for Inner {
    fn drop(&mut self) {
        let Ok(mut sessions) = self.sessions.lock() else {
            return; // poisoned: another thread panicked, nothing safe to do here
        };
        for session in sessions.iter_mut() {
            if let Some(child) = session.child.as_mut() {
                let _ = child.kill();
                // Reap it: a killed child left unwaited is a zombie holding its
                // slot until this process exits.
                let _ = child.wait();
            }
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
        PtyManager {
            inner: Arc::new(Inner {
                sessions: Mutex::new(Vec::new()),
                next_id: AtomicU64::new(1),
                now: Arc::new(medulla::clock::now_millis),
            }),
        }
    }

    /// Override the clock (tests).
    pub fn with_now(now: NowFn) -> Self {
        PtyManager {
            inner: Arc::new(Inner {
                sessions: Mutex::new(Vec::new()),
                next_id: AtomicU64::new(1),
                now,
            }),
        }
    }

    fn now(&self) -> i64 {
        (self.inner.now)()
    }
}

mod attention;
mod open;
mod screen;
mod session;
#[cfg(test)]
mod tests;

pub use screen::{ScreenCell, ScreenSnapshot};

mod types;
use types::Inner;
pub use types::NowFn;
pub use types::PtyManager;
