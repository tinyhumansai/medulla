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

/// Turn an MCP subprocess's grant into a provider-only parent handoff.
///
/// The embedded daemon may spawn several ACP sessions for one workflow. Each
/// session exchanges this parent capability for its own child grant immediately
/// before attaching the MCP server; none may inherit or share the parent token.
async fn nested_harness_env(
    env: &std::collections::HashMap<String, String>,
) -> Result<(std::collections::HashMap<String, String>, u8), crate::workflows::WorkflowError> {
    let mut nested = env.clone();
    nested.remove(crate::control_socket::MCP_PARENT_SOCKET_ENV);
    nested.remove(crate::control_socket::MCP_PARENT_GRANT_ENV);
    let Some((socket, token)) = crate::control_socket::grant_from_env(env) else {
        return Ok((nested, 0));
    };
    nested.remove(crate::control_socket::MCP_SOCKET_ENV);
    nested.remove(crate::control_socket::MCP_GRANT_ENV);

    #[cfg(unix)]
    {
        let client = crate::control_socket::ControlClient::connect(&socket, &token)
            .await
            .map_err(|error| {
                crate::workflows::WorkflowError::Engine(format!(
                    "cannot prepare nested fleet access: {error}"
                ))
            })?;
        let depth = client.hello().depth.saturating_add(1);
        nested.insert(
            crate::control_socket::MCP_PARENT_SOCKET_ENV.to_string(),
            socket.to_string_lossy().into_owned(),
        );
        nested.insert(
            crate::control_socket::MCP_PARENT_GRANT_ENV.to_string(),
            token,
        );
        Ok((nested, depth))
    }
    #[cfg(not(unix))]
    {
        let _ = (socket, token);
        Ok((nested, 0))
    }
}

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
    let (host_env, fleet_depth) = nested_harness_env(env).await?;
    settings.fleet_depth = fleet_depth;

    let host = LocalWorkflowHost::start(EmbeddedDaemonOptions {
        workspace: cwd.to_string_lossy().to_string(),
        default_provider: config.default_provider,
        model: (!config.default_model.is_empty()).then(|| config.default_model.clone()),
        // Without these, a workflow whose `agent` node selects a custom
        // harness preset would run onto an embedded daemon with an empty
        // preset list and be refused as "not configured on this host" even
        // though the operator has it configured right here.
        custom_harnesses: custom_harnesses.to_vec(),
        env: host_env,
        ..Default::default()
    })
    .map_err(crate::workflows::WorkflowError::Engine)?;

    let (sink, _fold) = folding_sink();
    let max_loop_iterations = settings.max_loop_iterations;
    let run_id = format!("run-{}", uuid::Uuid::new_v4());
    // A tool call reaching here came from a harness Medulla spawned, so the
    // grant in `env` names the session that asked for this run. Reporting
    // through it is what puts the run under that session in the operator's
    // rail instead of leaving it invisible until the record hits disk.
    let reporter = crate::workflows::RunReporter::start(env, id, &run_id);
    let mut services = HostServices::new(
        host.dispatch(),
        Arc::new(StoreWorkflowResolver::new(
            store.clone(),
            max_loop_iterations,
        )),
        std::collections::HashMap::new(),
    );
    if let Some(reporter) = &reporter {
        services = services.watching(reporter.progress_sink());
    }
    let context = RunContext {
        store,
        settings: Arc::new(settings),
        services,
        sink,
    };

    let outcome =
        crate::workflows::run_workflow(context, id, &run_id, run_input.trigger, run_input.inputs)
            .await;
    if let Some(reporter) = &reporter {
        // Reported from the outcome rather than from the record's own status
        // word, so a run that failed before it produced a record still settles
        // the row rather than leaving it running forever.
        let (status, detail) = match &outcome {
            Ok(record) => (
                crate::workflows::report::wire_status(record.status),
                record.summary.clone(),
            ),
            Err(error) => ("failed", Some(error.to_string())),
        };
        reporter.settled(status, detail);
    }
    outcome
}

/// Run one copilot authoring turn on this machine, with no TUI involved.
///
/// The headless counterpart to the Workflows pane. It exists for two reasons
/// beyond convenience: a copilot turn was previously reachable *only* through
/// the pane, so nothing outside a running terminal could prove the harness
/// actually receives its `workflow_*` tools — and when it does not, the failure
/// is a confident reply and an unchanged graph, which reads as success. This is
/// what the live test in `src/sdk/tests/live_copilot.rs` drives, and what an
/// operator runs to find out whether authoring works on their machine at all.
///
/// `target` names the workflow to revise; `None` is a create turn. `status`
/// receives the harness's progress frames — the same ones the pane draws — and
/// may be dropped to ignore them.
///
/// Forces ACP and preflights the MCP server for the same reason the pane does:
/// the legacy provider transport cannot attach an MCP server, so a turn that
/// silently fell back to it would produce an agent that can only discuss the
/// graph.
///
/// # Errors
///
/// Fails when workflows are disabled on this host, when the MCP preflight
/// fails, when the embedded host cannot start (no coding-agent CLI on `PATH`),
/// or when the turn itself does. A refused *edit* is not an error: the tool
/// told the agent why, and it says so in the reply.
pub async fn author_here(
    store: Arc<dyn crate::workflows::WorkflowStore>,
    config: &crate::config::WorkflowsConfig,
    cwd: &std::path::Path,
    target: Option<&str>,
    instruction: &str,
    status: Option<tokio::sync::mpsc::UnboundedSender<String>>,
) -> Result<crate::workflows::CopilotOutcome, crate::workflows::WorkflowError> {
    use crate::workflows::CopilotSession;

    if !config.enabled {
        return Err(crate::workflows::WorkflowError::Engine(
            "workflows are disabled on this host (workflows.enabled = false)".to_string(),
        ));
    }
    // Checked before the host starts, for the same reason `run_here` checks
    // `enabled`: standing up an embedded daemon needs a coding-agent CLI on
    // `PATH`, and revising a workflow that does not exist should not cost that.
    if let Some(id) = target {
        crate::workflows::store::require(store.as_ref(), id)?;
    }

    let mut env: std::collections::HashMap<String, String> = std::env::vars().collect();
    env.insert(
        crate::daemon::providers::HARNESS_PROTOCOL_ENV.to_string(),
        "acp".to_string(),
    );
    crate::mcp::preflight(&env, cwd).map_err(crate::workflows::WorkflowError::Engine)?;

    let host = LocalWorkflowHost::start(EmbeddedDaemonOptions {
        workspace: cwd.to_string_lossy().to_string(),
        default_provider: config.default_provider,
        model: (!config.default_model.is_empty()).then(|| config.default_model.clone()),
        env,
        ..Default::default()
    })
    .map_err(crate::workflows::WorkflowError::Engine)?;

    let session = CopilotSession {
        store,
        dispatch: host.dispatch(),
        worker_address: LOCAL_WORKER_ADDRESS.to_string(),
        provider: config.default_provider,
        model: (!config.default_model.is_empty()).then(|| config.default_model.clone()),
        // One-shot: a CLI invocation has no pane to be continuous with, and a
        // shared key would have each invocation inherit the last one's context
        // whether or not that is wanted.
        conversation: format!("author-{}", uuid::Uuid::new_v4()),
        // A one-shot invocation is not resuming a pane's thread. Passing a
        // recap here would mean deciding *which* pane's, and there is no
        // answer to that from a command line.
        recap: None,
    };
    match target {
        Some(id) => session.turn(id, instruction, status).await,
        None => session.create(instruction, status).await,
    }
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
    crate::mcp::preflight(&env, cwd).map_err(crate::workflows::WorkflowError::Engine)?;

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
