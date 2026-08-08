//! Running-task registration: claiming the per-task `running` record.

use super::super::types::{DaemonRuntime, RunningTask};

impl DaemonRuntime {
    /// Claim `key` for `task`, returning whether the claim succeeded.
    ///
    /// The check and the insert are one locked operation on purpose. Each frame
    /// is handled by its own spawned task, so two frames carrying the same
    /// sender and task id can reach this at the same time; with a separate
    /// `contains_key` check both would pass it, the second would overwrite the
    /// first's [`RunningTask`], and the first admission guard's drop would then
    /// remove the shared key — leaving a harness running that no abort, input,
    /// or screen frame can reach.
    pub(in crate::daemon) fn register_running(&self, key: &str, task: RunningTask) -> bool {
        use std::collections::hash_map::Entry;
        match self.inner.running.lock().unwrap().entry(key.to_string()) {
            Entry::Occupied(_) => false,
            Entry::Vacant(slot) => {
                slot.insert(task);
                true
            }
        }
    }
}
