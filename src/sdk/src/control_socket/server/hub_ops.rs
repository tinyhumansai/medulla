//! [`FleetOps`] over a live hub handle.
//!
//! The slot is read on every call rather than resolved once at construction.
//! Two reasons, both of which produced a wrong answer when this cached: the hub
//! fills its slot asynchronously after boot, so a handle captured at startup is
//! usually absent; and a relogin *replaces* the handle, so a captured one goes
//! stale mid-session while still looking valid.

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::hub::{HubHandle, RunError, TaskOutcome, TaskRequest};

use super::super::types::{FleetOps, FleetWorker};

/// The shared slot the hub fills once it connects.
///
/// Structurally identical to [`crate::runtime::openhuman::HubSlot`], and the
/// same type — named here so this module does not depend on the runtime.
pub type HubSlot = Arc<Mutex<Option<HubHandle>>>;

/// Fleet defaults for dispatches that name no worker.
#[derive(Debug, Clone, Default)]
pub struct FleetDefaults {
    /// The worker address to use when neither the caller nor the roster's
    /// selection names one. Usually this device's own host address.
    pub worker_address: Option<String>,
}

/// [`FleetOps`] backed by whatever handle is in the slot right now.
pub struct HubFleetOps {
    slot: HubSlot,
    defaults: FleetDefaults,
}

impl HubFleetOps {
    /// Serve the fleet reachable through `slot`.
    pub fn new(slot: HubSlot, defaults: FleetDefaults) -> Self {
        HubFleetOps { slot, defaults }
    }

    /// The handle currently in the slot, if the hub has connected.
    fn handle(&self) -> Option<HubHandle> {
        self.slot.lock().ok()?.clone()
    }
}

#[async_trait::async_trait]
impl FleetOps for HubFleetOps {
    fn workers(&self) -> Option<Vec<FleetWorker>> {
        let handle = self.handle()?;
        let running = handle.activity().running_by_agent();
        Some(
            handle
                .list()
                .into_iter()
                .map(|worker| {
                    let running = running.get(&worker.id).cloned().unwrap_or_default();
                    // `control` reports who holds the harness, and it is only
                    // set when a *person* does — the orchestrator holding it is
                    // the unmarked common case.
                    let held = worker.control.is_operator();
                    FleetWorker {
                        id: worker.id,
                        address: worker.address,
                        harness: worker.harness,
                        label: worker.label,
                        roles: worker.roles,
                        workspace: worker.workspace,
                        selected: worker.selected,
                        held,
                        held_reason: worker.control_reason,
                        running,
                    }
                })
                .collect(),
        )
    }

    fn default_worker(&self) -> Option<String> {
        // The operator's own selection first: it is the worker every other part
        // of the TUI treats as "here", so a dispatch that names none should land
        // in the same place a typed instruction would.
        if let Some(handle) = self.handle() {
            if let Some(selected) = handle.list().into_iter().find(|worker| worker.selected) {
                return Some(selected.address);
            }
        }
        self.defaults.worker_address.clone()
    }

    async fn dispatch(
        &self,
        request: TaskRequest,
        status: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<TaskOutcome, RunError> {
        let Some(handle) = self.handle() else {
            return Err(RunError::Transport(
                "the hub is not connected yet, so there is nothing to dispatch through".to_string(),
            ));
        };
        handle.task_runner().run(request, status).await
    }

    fn abort(&self, abort_id: &str) {
        if let Some(handle) = self.handle() {
            handle.task_runner().abort_task(abort_id);
        }
    }
}
