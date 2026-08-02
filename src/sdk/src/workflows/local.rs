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
use crate::hub::{RunError, TaskOutcome, TaskRequest, TaskRunner};

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

/// Run the workflow `id` on this machine, start to finish.
///
/// Everything a caller needs assembled in one place: settings from config, a
/// loopback host for `agent` nodes to dispatch to, and the run itself. The
/// `medulla workflow run` command builds its own because it has more to say
/// about run ids and progress; this is for callers that want the plain thing —
/// today the copilot's `workflow_run` tool.
///
/// The host is held for the whole run and dropped with it, which unbinds the
/// loopback endpoints so a later run can bind them again.
///
/// # Errors
///
/// `run_input` carries both halves of what a caller supplies for this run — the
/// free-form trigger payload and the values for the workflow's declared inputs.
/// They travel as the engine's own [`RunInput`](tinyflows::engine::RunInput)
/// rather than two parameters because they are one idea, and because this
/// function already takes as many distinct arguments as it usefully can.
///
/// # Errors
///
/// Fails when no coding-agent CLI is installed, when the workflow or the host
/// is disabled, or when the run itself does.
pub async fn run_here(
    store: Arc<dyn crate::workflows::WorkflowStore>,
    config: &crate::config::WorkflowsConfig,
    custom_harnesses: &[crate::config::CustomHarnessConfig],
    env: &std::collections::HashMap<String, String>,
    cwd: &std::path::Path,
    id: &str,
    run_input: tinyflows::engine::RunInput,
) -> Result<crate::workflows::RunRecord, crate::workflows::WorkflowError> {
    use crate::flow_engine::{folding_sink, CapabilitySettings, HostServices};
    use crate::workflows::{RunContext, StoreWorkflowResolver};

    // Checked before the host, not after: starting a host requires a
    // coding-agent CLI on `PATH`, a cost a disabled workflow (or a host with
    // workflows turned off) should never pay just to be told no. Every path
    // here still runs `run_workflow`'s own checks too — this is an early exit
    // for the common refusal, not a replacement for the authoritative one.
    if !config.enabled {
        return Err(crate::workflows::WorkflowError::Engine(
            "workflows are disabled on this host (workflows.enabled = false)".to_string(),
        ));
    }
    let workflow = crate::workflows::store::require(store.as_ref(), id)?;
    if !workflow.enabled {
        return Err(crate::workflows::WorkflowError::Engine(format!(
            "workflow '{id}' is disabled"
        )));
    }

    let home = crate::home::medulla_home(env);
    let mut settings = CapabilitySettings::from_config(config, &home);
    settings.workspace = cwd.to_string_lossy().to_string();
    if settings.default_worker_address.trim().is_empty() {
        settings.default_worker_address = LOCAL_WORKER_ADDRESS.to_string();
    }

    let host = LocalWorkflowHost::start(EmbeddedDaemonOptions {
        workspace: cwd.to_string_lossy().to_string(),
        default_provider: config.default_provider,
        model: (!config.default_model.is_empty()).then(|| config.default_model.clone()),
        // Without these, a workflow whose `agent` node selects a custom
        // harness preset would run onto an embedded daemon with an empty
        // preset list and be refused as "not configured on this host" even
        // though the operator has it configured right here.
        custom_harnesses: custom_harnesses.to_vec(),
        ..Default::default()
    })
    .map_err(crate::workflows::WorkflowError::Engine)?;

    let (sink, _fold) = folding_sink();
    let context = RunContext {
        store: store.clone(),
        settings: Arc::new(settings),
        services: HostServices {
            dispatch: host.dispatch(),
            resolver: Arc::new(StoreWorkflowResolver::new(store)),
            http_credentials: std::collections::HashMap::new(),
        },
        sink,
    };

    let run_id = format!("run-{}", uuid::Uuid::new_v4());
    crate::workflows::run_workflow(context, id, &run_id, run_input.trigger, run_input.inputs).await
}

/// Review a workflow against its own history, on this machine.
///
/// The evolution counterpart to [`run_here`], and it starts the same embedded
/// host for the same reason: a review is a harness turn, and the CLI has no
/// daemon to borrow one from.
///
/// The trigger is passed in rather than inferred. `medulla workflow evolve`
/// with no run is a manual review; with one it is the failure pass, which leads
/// with that run.
///
/// The workflow must exist, be enabled, and workflow evolution must be enabled.
/// This starts an embedded daemon bound to [`LOCAL_WORKER_ADDRESS`] and forces
/// ACP so the review can use the restricted workflow MCP surface.
///
/// # Errors
///
/// Returns an error when those preconditions fail, MCP support is unavailable,
/// the embedded host cannot start, or the review turn fails.
pub async fn evolve_here(
    store: Arc<dyn crate::workflows::WorkflowStore>,
    config: &crate::config::WorkflowsConfig,
    cwd: &std::path::Path,
    id: &str,
    trigger: crate::workflows::evolve::EvolveTrigger,
) -> Result<crate::workflows::evolve::EvolveOutcome, crate::workflows::WorkflowError> {
    use crate::workflows::evolve::{EvolveConfig, EvolveSession};

    let settings = EvolveConfig::from_config(config);
    if !settings.enabled {
        return Err(crate::workflows::WorkflowError::Engine(
            "workflow evolution is disabled on this host".to_string(),
        ));
    }
    // Checked before the host starts, for the same reason `run_here` checks
    // `enabled`: standing up an embedded daemon needs a coding-agent CLI on
    // `PATH`, and a workflow that does not exist should not cost that.
    let workflow = crate::workflows::store::require(store.as_ref(), id)?;
    if !workflow.enabled {
        return Err(crate::workflows::WorkflowError::Engine(format!(
            "workflow '{id}' is disabled"
        )));
    }

    // Evolution depends on the restricted workflow MCP surface. The legacy
    // provider transport cannot attach MCP servers, so an otherwise successful
    // review would be unable to record a note or proposal.
    let mut env: std::collections::HashMap<String, String> = std::env::vars().collect();
    env.insert(
        crate::daemon::providers::HARNESS_PROTOCOL_ENV.to_string(),
        "acp".to_string(),
    );
    crate::workflows::mcp::preflight(&env, cwd).map_err(crate::workflows::WorkflowError::Engine)?;

    let host = LocalWorkflowHost::start(EmbeddedDaemonOptions {
        workspace: cwd.to_string_lossy().to_string(),
        default_provider: config.default_provider,
        model: (!config.default_model.is_empty()).then(|| config.default_model.clone()),
        env,
        ..Default::default()
    })
    .map_err(crate::workflows::WorkflowError::Engine)?;

    let session = EvolveSession {
        store,
        dispatch: host.dispatch(),
        worker_address: LOCAL_WORKER_ADDRESS.to_string(),
        provider: config.default_provider,
        model: (!config.default_model.is_empty()).then(|| config.default_model.clone()),
        // One-shot: a CLI invocation has no pane to be continuous with, and
        // sharing a conversation key across invocations would have each review
        // inherit the last one's context whether or not that is wanted.
        conversation: format!("evolve-{}-{}", id, uuid::Uuid::new_v4()),
        config: settings,
    };
    session.evolve(id, trigger, None).await
}

/// A copilot dispatch that starts its loopback host per turn.
///
/// The authoring copilot has to run *here*, on the machine holding the workflow
/// store: its edits land through the `medulla-workflows` MCP tools, which are
/// attached to the session by [`LocalWorkflowHost`] and point at this host's own
/// store. Dispatching the turn to a remote worker instead would run the prompt
/// somewhere its edits could never reach the graph the orchestrator asked about.
///
/// Per turn, not per process: the host binds two loopback addresses and holds a
/// daemon draining them, and keeping that alive between turns would occupy the
/// endpoints the TUI's own copilot pane and every workflow run also bind. Turns
/// are minutes apart at best, so starting one costs nothing that matters.
pub struct LocalCopilotDispatch {
    /// How the per-turn daemon is configured — workspace, provider, model, env.
    options: EmbeddedDaemonOptions,
}

impl LocalCopilotDispatch {
    /// A dispatch that starts a host per turn with `options`.
    pub fn new(options: EmbeddedDaemonOptions) -> Self {
        Self { options }
    }
}

#[async_trait::async_trait]
impl HarnessDispatch for LocalCopilotDispatch {
    async fn dispatch(&self, request: TaskRequest) -> Result<TaskOutcome, RunError> {
        self.dispatch_with_status(request, None).await
    }

    /// Start a host, run the turn on it, and drop the host with the turn.
    ///
    /// # Errors
    ///
    /// A host that cannot start — no coding-agent CLI installed, an address
    /// already bound — is reported as a transport failure rather than awaited.
    /// The caller is answering a request the backend is holding a ten-minute
    /// promise for, and "this host cannot author" is worth saying immediately.
    async fn dispatch_with_status(
        &self,
        request: TaskRequest,
        status: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<TaskOutcome, RunError> {
        let host = LocalWorkflowHost::start(self.options.clone()).map_err(RunError::Transport)?;
        host.dispatch().dispatch_with_status(request, status).await
    }
}

#[cfg(test)]
#[path = "local_tests.rs"]
mod tests;
