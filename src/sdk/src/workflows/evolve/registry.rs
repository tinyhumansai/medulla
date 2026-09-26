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

/// Workflows with a pass in flight.
fn claimed() -> &'static Mutex<HashSet<String>> {
    static CLAIMED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    CLAIMED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// A claim key unique to one workflow store and workflow.
fn claim_key(store_scope: &str, workflow_id: &str) -> String {
    format!("{store_scope}:workflow:{workflow_id}")
}

/// Held for the duration of a pass; releases the workflow on drop.
///
/// A guard rather than an explicit release so a pass that panics or is dropped
/// mid-await does not leave a workflow permanently unable to evolve.
pub struct EvolveGuard {
    key: String,
}

impl EvolveGuard {
    /// Claim `workflow_id` within `store_scope`, or `None` when it is busy.
    pub fn claim(store_scope: &str, workflow_id: &str) -> Option<Self> {
        let key = claim_key(store_scope, workflow_id);
        let mut claimed = claimed().lock().expect("evolve registry lock");
        if !claimed.insert(key.clone()) {
            return None;
        }
        Some(Self { key })
    }
}

impl Drop for EvolveGuard {
    fn drop(&mut self) {
        claimed()
            .lock()
            .expect("evolve registry lock")
            .remove(&self.key);
    }
}

/// Whether a pass is running for `workflow_id` in `store_scope`.
pub fn is_evolving(store_scope: &str, workflow_id: &str) -> bool {
    claimed()
        .lock()
        .expect("evolve registry lock")
        .contains(&claim_key(store_scope, workflow_id))
}
