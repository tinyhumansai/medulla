//! One evolution pass per workflow at a time.
//!
//! A workflow that is failing is usually failing repeatedly, and the trigger is
//! "a run ended badly". Without a claim, a workflow failing ten times in a
//! minute would start ten harness sessions to reach the same conclusion ten
//! times over.
//!
//! Process-local, like [`crate::workflows::run::registry`] and for the same
//! reason: there is no control channel between two Medulla processes. That is
//! the right scope anyway — the thing being debounced is *this* host spending
//! its own harness capacity.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use crate::workflows::WorkflowId;

/// Workflows with a pass in flight.
fn claimed() -> &'static Mutex<HashSet<WorkflowId>> {
    static CLAIMED: OnceLock<Mutex<HashSet<WorkflowId>>> = OnceLock::new();
    CLAIMED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Held for the duration of a pass; releases the workflow on drop.
///
/// A guard rather than an explicit release so a pass that panics or is dropped
/// mid-await does not leave a workflow permanently unable to evolve.
pub struct EvolveGuard {
    workflow_id: WorkflowId,
}

impl EvolveGuard {
    /// Claim `workflow_id`, or `None` when a pass is already running for it.
    pub fn claim(workflow_id: &str) -> Option<Self> {
        let mut claimed = claimed().lock().expect("evolve registry lock");
        if !claimed.insert(workflow_id.to_string()) {
            return None;
        }
        Some(Self {
            workflow_id: workflow_id.to_string(),
        })
    }
}

impl Drop for EvolveGuard {
    fn drop(&mut self) {
        claimed()
            .lock()
            .expect("evolve registry lock")
            .remove(&self.workflow_id);
    }
}

/// Whether a pass is running for `workflow_id` in this process.
pub fn is_evolving(workflow_id: &str) -> bool {
    claimed()
        .lock()
        .expect("evolve registry lock")
        .contains(workflow_id)
}
