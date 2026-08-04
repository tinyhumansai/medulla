//! [`FleetOps`] over a live hub handle.
//!
//! The slot is read on every call rather than resolved once at construction.
//! Two reasons, both of which produced a wrong answer when this cached: the hub
//! fills its slot asynchronously after boot, so a handle captured at startup is
//! usually absent; and a relogin *replaces* the handle, so a captured one goes
//! stale mid-session while still looking valid.

use tokio::sync::mpsc;

use crate::hub::{ActivityLog, HubHandle, RunError, TaskOutcome, TaskRequest};
use crate::protocol::WorkflowAdvert;

use super::super::types::{FleetOps, FleetWorker};
use super::types::{FleetDefaults, HubSlot};

/// Forward a dispatch's status frames to its control-plane poller.
///
/// The production task runner already writes each frame to the activity log.
/// This channel exists only to give `fleet_result` its own progress tail.
pub(super) fn tee_status(
    onward: Option<mpsc::UnboundedSender<String>>,
) -> mpsc::UnboundedSender<String> {
    let (tee, mut frames) = mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        while let Some(line) = frames.recv().await {
            if let Some(onward) = &onward {
                // A closed receiver means the poller gave up. The task runner's
                // independent activity path still records later frames.
                let _ = onward.send(line);
            }
        }
    });
    tee
}

/// Record how a dispatch ended, in the frame kinds the Agents view renders.
pub(super) fn record_outcome(
    activity: &ActivityLog,
    task_id: &str,
    outcome: &Result<TaskOutcome, RunError>,
) {
    // Normal worker replies and errors are recorded by the hub pump before the
    // runner settles. This is only the fallback for locally generated outcomes
    // such as transport failures, aborts, and timeouts that produced no frame.
    if activity.has_terminal(task_id) {
        return;
    }
    let (kind, content) = match outcome {
        Ok(result) => ("reply", result.reply.clone()),
        Err(error) => ("error", error.to_string()),
    };
    activity.observed(task_id, kind, &content, crate::clock::now_millis());
}

/// The Agents-view key for a task dispatched through the control plane.
///
/// It carries the server-minted abort handle, not the worker-facing dedupe id,
/// so the operator can cancel the row even after its originating grant ends.
pub(super) fn activity_key(request: &TaskRequest) -> String {
    match request.cycle_id.as_deref() {
        Some(cycle) => format!("{cycle}/t:{}", request.abort_id),
        None => request.abort_id.clone(),
    }
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

    /// The roster id for a worker address, for attributing activity to a lane.
    ///
    /// Falls back to the address itself when the roster does not name it: the
    /// Agents view would rather show a task against an unfamiliar id than lose
    /// it to no lane at all.
    fn roster_id(&self, handle: &HubHandle, address: &str) -> String {
        handle
            .list()
            .into_iter()
            .find(|worker| worker.address == address)
            .map(|worker| worker.id)
            .unwrap_or_else(|| address.to_string())
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
                        // The path only: this surface renders a directory, and
                        // the workspace type has no reader here.
                        workspace: worker.workspace.map(|workspace| workspace.path),
                        selected: worker.selected,
                        held,
                        held_reason: worker.control_reason,
                        running,
                        workflows: Vec::new(),
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

    async fn worker_workflows(&self, worker: &str) -> Result<Vec<WorkflowAdvert>, RunError> {
        let Some(handle) = self.handle() else {
            return Err(RunError::Transport(
                "the hub is not connected yet, so workflow capabilities are unavailable"
                    .to_string(),
            ));
        };
        handle
            .task_runner()
            .capabilities(worker)
            .await
            .map(|capabilities| capabilities.workflows)
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

        // Recorded into the same activity log the orchestrator's own dispatches
        // use, so a task a harness started appears in the Agents view beside the
        // ones an operator started. Work running on somebody's machine that
        // their UI does not show is work they cannot see, cannot attribute, and
        // cannot stop — which is the worst property this surface could have.
        //
        // Attributed before the dispatch, for the reason the inbound path gives:
        // a frame arriving before its dispatch is recorded would be orphaned
        // onto no worker at all.
        let activity = handle.activity();
        let agent_id = self.roster_id(&handle, &request.worker_address);
        let task_id = activity_key(&request);
        let wire_task_id = request.task_id.clone();
        activity.dispatched_as(&wire_task_id, &task_id, &agent_id);
        activity.observed(
            &wire_task_id,
            "status",
            "dispatching task",
            crate::clock::now_millis(),
        );

        let tee = tee_status(status);
        let runner = handle.task_runner();
        let (capabilities, abort) = runner
            .capabilities_for_dispatch(&request.worker_address, &request.abort_id)
            .await;
        let screen_kill = match capabilities {
            Ok(capabilities) => capabilities.screen_kill,
            Err(RunError::Aborted) => {
                let outcome = Err(RunError::Aborted);
                record_outcome(&activity, &wire_task_id, &outcome);
                return outcome;
            }
            Err(_) => false,
        };
        let outcome = runner
            .run_negotiated(request, Some(tee), screen_kill, Some(abort), Some(task_id))
            .await;
        record_outcome(&activity, &wire_task_id, &outcome);
        outcome
    }

    fn abort(&self, abort_id: &str) -> bool {
        if let Some(handle) = self.handle() {
            handle.task_runner().abort_task(abort_id)
        } else {
            false
        }
    }
}
