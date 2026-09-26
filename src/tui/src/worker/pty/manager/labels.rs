//! Keeping each session's Codex thread name current between turns.
//!
//! Codex persists a `/rename` in `session_index.jsonl` rather than emitting an
//! OSC window title, so the terminal stream never carries the new name. The
//! transcript executor reads that file once per located turn — which leaves a
//! session the operator is holding, has been handed back, or is merely
//! retained showing the old name until the next delegated turn happens to
//! start. A timer per live session fixes that at the only cost that matters:
//! one small file read on a thread. It is the Codex counterpart of the
//! [checkout poller](super::checkout) beside it.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use super::super::handle::SessionHandle;
use super::PtyManager;

/// How often a live session's index-backed thread name is re-read.
///
/// Two seconds, like the checkout poller: a rename happens a handful of times
/// an hour, and re-reading a small JSONL file is cheap compared to the `git`
/// the checkout poller runs at the same rate.
const LABEL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

impl PtyManager {
    /// Re-read `handle`'s Codex thread name on a timer for as long as that
    /// session lives.
    ///
    /// Both references are non-owning: a poller cannot keep either the manager
    /// or a reaped session alive.
    ///
    /// The environment is a snapshot taken from the `LaunchSpec`, not a borrow
    /// of it: the poller outlives `open`, and a handful of variables cloned once
    /// per Codex session is the same shape the executor's held env already is.
    /// A session a turn has located is looked up by its harness session id; one
    /// nothing has located — an operator-created session, which never passes
    /// through the transcript executor — falls back to the newest Codex rollout
    /// rooted at this session's cwd.
    pub(super) fn spawn_codex_label_poller(
        &self,
        handle: Weak<SessionHandle>,
        env: HashMap<String, String>,
    ) {
        let inner = Arc::downgrade(&self.inner);
        std::thread::spawn(move || loop {
            std::thread::sleep(LABEL_INTERVAL);
            let (Some(handle), true) = (handle.upgrade(), inner.upgrade().is_some()) else {
                return;
            };
            // An exited session's name is frozen at whatever it last was: the
            // row is kept for the operator to read, and a dead session cannot be
            // renamed.
            if !handle.is_running() {
                return;
            }
            let label = match handle.session_id() {
                Some(session_id) => medulla::session_history::codex_thread_label(&env, &session_id),
                None => medulla::session_history::codex_thread_label_for_cwd(&env, handle.cwd()),
            };
            if let Some(label) = label {
                handle.record_thread_name(label);
            }
        });
    }
}
