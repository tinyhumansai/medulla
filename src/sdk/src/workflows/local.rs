//! Running workflows on this machine, with no orchestrator involved.
//!
//! A workflow's `agent` nodes dispatch task frames, and a task frame needs
//! somewhere to go. In the deployed shape that is the hub relaying to a remote
//! worker; for `medulla workflow run` on a laptop it is the same protocol
//! looped back through an in-process bridge to an embedded daemon that spawns
//! the coding CLIs already on `PATH`.
//!
//! Nothing here is a simulation — the frames, the correlation, the daemon, and
//! the harness processes are the real ones. The only difference from a
//! distributed run is that both ends are in this process.

use std::sync::Arc;
use std::time::Duration;

use crate::bridge::LocalBridgeNetwork;
use crate::daemon::embedded::{EmbeddedDaemon, EmbeddedDaemonOptions};
use crate::flow_engine::caps::dispatch::{HarnessDispatch, TaskRunnerDispatch};
use crate::hub::TaskRunner;

/// How often the loopback bridge is drained. Short, because both ends are in
/// this process and the only cost is a poll on an in-memory queue.
const LOCAL_POLL: Duration = Duration::from_millis(20);

/// The bridge address the workflow runner dispatches from.
const ORCHESTRATOR_ADDRESS: &str = "workflow-runner";

/// The bridge address the embedded worker listens on. Workflows whose nodes
/// name no `agent_ref` dispatch here.
pub const LOCAL_WORKER_ADDRESS: &str = "workflow-local-worker";

/// A loopback host: an embedded daemon plus the dispatch that reaches it.
///
/// Held for the lifetime of the runs it serves. Dropping it unbinds both
/// endpoints and stops the daemon's drain loop.
pub struct LocalWorkflowHost {
    dispatch: Arc<dyn HarnessDispatch>,
    /// Kept alive so the worker keeps draining; not otherwise read.
    _daemon: EmbeddedDaemon,
}

impl LocalWorkflowHost {
    /// Start a loopback host serving tasks from the coding CLIs on `PATH`.
    ///
    /// # Errors
    ///
    /// Fails when no coding-agent CLI is installed, or when either bridge
    /// address cannot be bound — both are situations an operator has to see,
    /// rather than a host that starts and then rejects every task.
    pub fn start(options: EmbeddedDaemonOptions) -> Result<Self, String> {
        let network = LocalBridgeNetwork::new();
        let worker = network.bind(LOCAL_WORKER_ADDRESS)?;
        let orchestrator = network.bind(ORCHESTRATOR_ADDRESS)?;

        let daemon = EmbeddedDaemon::start(Arc::new(worker), LOCAL_WORKER_ADDRESS, options)?;
        let runner = Arc::new(TaskRunner::start(Arc::new(orchestrator), LOCAL_POLL));

        Ok(Self {
            dispatch: Arc::new(TaskRunnerDispatch::new(runner)),
            _daemon: daemon,
        })
    }

    /// The dispatch to hand to a run's capabilities.
    pub fn dispatch(&self) -> Arc<dyn HarnessDispatch> {
        self.dispatch.clone()
    }

    /// Stop whatever this host currently has in flight.
    ///
    /// Scoped to this host, which is what makes it safe: a copilot pane holds
    /// one of these per conversation, so stopping "everything here" is stopping
    /// the turn the operator is watching and nothing else.
    pub fn abort(&self) {
        self.dispatch.abort_in_flight();
    }
}
