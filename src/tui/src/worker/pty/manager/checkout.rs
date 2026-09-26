//! Keeping each session's repository, worktree, and branch current.
//!
//! The checkout a harness is pointed at is not a fact about its birth. An agent
//! whose first act is `git switch -c fix/thing` — which is most of them, under
//! a worktree-per-task workflow — spends the rest of its life on a branch the
//! launch snapshot never heard of, and the rail went on naming the branch it
//! started from. A timer per live session fixes that at the only cost that
//! matters: `git` runs off the render path.
//!
//! Deliberately slow, and deliberately per session. Slow because a branch
//! changes a handful of times an hour where a screen changes many times a
//! second — the [attention poller](super::attention) beside it samples five
//! times a second, and copying that rate here would be three forks per session
//! per 200ms for an answer that is almost always the same one. Per session
//! because that is what lets a poller die with the session it watches, exactly
//! as the attention one does.

use std::sync::{Arc, Weak};

use super::super::checkout::inspect;
use super::super::handle::SessionHandle;
use super::PtyManager;

/// How often a live session's checkout is re-read.
///
/// Two seconds: fast enough that an operator watching an agent cut a branch
/// sees the rail follow it within a glance, slow enough that four sessions cost
/// six `git` invocations a second between them rather than sixty.
const CHECKOUT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

impl PtyManager {
    /// Re-read `handle`'s checkout on a timer for as long as that session lives.
    ///
    /// Both references are non-owning: a poller cannot keep either the manager
    /// or a reaped session alive.
    pub(super) fn spawn_checkout_poller(&self, handle: Weak<SessionHandle>) {
        let inner = Arc::downgrade(&self.inner);
        std::thread::spawn(move || loop {
            std::thread::sleep(CHECKOUT_INTERVAL);
            let (Some(handle), true) = (handle.upgrade(), inner.upgrade().is_some()) else {
                return;
            };
            // An exited session's checkout is frozen at whatever it last was:
            // the row is being kept for the operator to read, and re-reading a
            // directory nothing is working in any more can only make it
            // disagree with the transcript beside it.
            if !handle.is_running() {
                return;
            }
            handle.set_checkout(inspect::resolve(handle.cwd()));
        });
    }
}
