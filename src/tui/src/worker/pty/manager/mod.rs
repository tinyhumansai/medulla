//! [`PtyManager`] — owns every live harness PTY and its terminal emulator.
//!
//! One session is: a real `claude`/`codex`/`opencode` child on a pseudo-terminal,
//! a reader thread draining the master into a [`vt100::Parser`], and the write
//! half kept open so keystrokes and injected peer prompts can reach the child.
//!
//! Split so no file exceeds the repo's 500-line ceiling: [`open`] launches a
//! harness on a fresh pty and drains it, [`session`] is the bookkeeping every
//! other caller reads, and [`screen`] is the emulator surface the UI renders.
//!
//! The reader runs on a **blocking thread**, not a tokio task: `portable-pty`'s
//! reader is a synchronous `Read` with no async variant, and parking it on the
//! async runtime would occupy a worker forever. It feeds the shared emulator and
//! exits when the master closes.

use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use super::types::PtySession;

/// Read buffer for the PTY master, sized for a full-screen redraw burst.
const BUF_LEN: usize = 8192;

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

/// A clock in epoch ms (injectable for tests).
pub type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Owns the live harness sessions the worker TUI renders.
///
/// Cheap to clone (an `Arc`), so the daemon's inbound-frame path and the render
/// loop share one.
#[derive(Clone)]
pub struct PtyManager {
    inner: Arc<Inner>,
}

struct Inner {
    /// Sessions in open order, so the list does not reshuffle under the cursor.
    sessions: Mutex<Vec<PtySession>>,
    next_id: AtomicU64,
    now: NowFn,
}

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

mod open;
mod screen;
mod session;

pub use screen::{ScreenCell, ScreenSnapshot};
