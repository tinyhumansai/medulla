//! The in-flight harness dispatch registry.
//!
//! A run started in this process dispatches its `agent` nodes onto a
//! [`LocalWorkflowHost`](crate::workflows::local) — an embedded daemon the run
//! owns and drops with itself. That host is nobody else's business: it does not
//! register with the outer control plane, so the fleet roster a
//! `worker.list` returns has no row for it and no `wf:{run}:…` task id in any
//! `running` list. Asking the fleet what a locally-started run is doing
//! therefore answers "nothing", however busy the run is.
//!
//! The dispatcher that *does* know is the capability itself. This is where it
//! says so: [`record`] is held across the dispatch await and dropped when the
//! harness replies, so [`in_flight`] is exactly the set of sessions this
//! process has out at this instant.
//!
//! Process-local, like [`super::registry`] and for the same reason — a run
//! executing in the TUI is invisible here, which is why a reader joins this
//! with the fleet roster rather than choosing between them.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::workflows::RunId;

/// One harness session this process has dispatched and not yet had back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InFlightDispatch {
    /// The dispatched task id, `wf:{run}:{route}#{sequence}`.
    pub task_id: String,
    /// The worker address it went to.
    pub worker: String,
    /// The harness it resolved to, or empty when the node left the choice to
    /// the worker's own configured harness.
    pub harness: String,
    /// The directory the run works in, when the host set one.
    pub workspace: Option<String>,
}

/// Dispatches outstanding per run, in this process.
type Registry = Mutex<HashMap<RunId, Vec<InFlightDispatch>>>;

/// The process-wide registry.
fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A recorded dispatch, withdrawn when dropped.
///
/// An RAII guard rather than a matched pair of calls because the dispatch await
/// can be cancelled — a run being dropped mid-node is the ordinary shape of
/// `workflow_run_cancel` — and a leaked entry would have the tool reporting a
/// harness session that ended minutes ago.
pub struct DispatchGuard {
    run_id: RunId,
    task_id: String,
}

/// Record `dispatch` as in flight for `run_id` until the guard drops.
#[must_use]
pub fn record(run_id: &str, dispatch: InFlightDispatch) -> DispatchGuard {
    let guard = DispatchGuard {
        run_id: run_id.to_string(),
        task_id: dispatch.task_id.clone(),
    };
    registry()
        .lock()
        .expect("dispatch registry lock")
        .entry(run_id.to_string())
        .or_default()
        .push(dispatch);
    guard
}

impl Drop for DispatchGuard {
    fn drop(&mut self) {
        let mut runs = match registry().lock() {
            Ok(runs) => runs,
            // Another dispatch panicked while holding the lock. Nothing safe to
            // do here; the stale entry is the lesser harm.
            Err(_) => return,
        };
        let Some(dispatches) = runs.get_mut(&self.run_id) else {
            return;
        };
        // Task ids are unique per run — the sequence is a per-run counter — so
        // removing the first match removes this guard's own entry.
        if let Some(at) = dispatches
            .iter()
            .position(|dispatch| dispatch.task_id == self.task_id)
        {
            dispatches.remove(at);
        }
        // Keep the map from growing one empty vector per run the process ever
        // ran.
        if dispatches.is_empty() {
            runs.remove(&self.run_id);
        }
    }
}

/// The harness sessions `run_id` has out right now, in this process.
///
/// Empty for a run executing elsewhere, which proves nothing about it — see the
/// module docs.
pub fn in_flight(run_id: &str) -> Vec<InFlightDispatch> {
    registry()
        .lock()
        .expect("dispatch registry lock")
        .get(run_id)
        .cloned()
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "dispatches_tests.rs"]
mod tests;
