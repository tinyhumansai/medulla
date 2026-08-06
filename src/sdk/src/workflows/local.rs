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

/// One local run, described before it starts.
///
/// A struct rather than a parameter list because starting a run needs seven
/// distinct things and two of them are string-ish: assembled by name, a caller
/// cannot silently transpose `cwd` and `workflow_id`.
///
/// `input` carries both halves of what a caller supplies — the free-form
/// trigger payload and the values for the workflow's declared inputs — as the
/// engine's own [`RunInput`](tinyflows::engine::RunInput), because they are one
/// idea.
pub struct LocalRun<'a> {
    /// The store the definition comes from and the run record goes to.
    pub store: Arc<dyn crate::workflows::WorkflowStore>,
    /// The host's workflow settings.
    pub config: &'a crate::config::WorkflowsConfig,
    /// The custom harness presets this machine has configured.
    pub custom_harnesses: &'a [crate::config::CustomHarnessConfig],
    /// The environment the embedded daemon and its harnesses inherit.
    pub env: &'a std::collections::HashMap<String, String>,
    /// The workspace the run executes in.
    pub cwd: &'a std::path::Path,
    /// The workflow to run.
    pub workflow_id: &'a str,
    /// The trigger payload and declared-input values.
    pub input: tinyflows::engine::RunInput,
    /// Where progress events go, when someone is watching.
    ///
    /// `None` folds them into a snapshot nobody reads, which is what a caller
    /// that only wants the final record needs.
    pub sink: Option<crate::flow_engine::WorkEventSink>,
}

/// A run that has been admitted and is executing in the background.
///
/// The run id is available *before* the run settles, which is the whole point:
/// a caller that cannot hold a request open for an hour can hand the id back
/// and let the operator (or the model) poll `workflow_run_get`.
pub struct StartedRun {
    /// The run's id, already written to the store as a `running` record.
    pub run_id: String,
    /// The workflow this run is of.
    pub workflow_id: String,
    /// The detached task executing it.
    ///
    /// Dropping this does *not* stop the run — a `JoinHandle` detaches on drop,
    /// which is exactly the durability this shape exists to give. Cancelling
    /// goes through [`crate::workflows::run::cancel`].
    join: tokio::task::JoinHandle<
        Result<crate::workflows::RunRecord, crate::workflows::WorkflowError>,
    >,
}

impl StartedRun {
    /// Wait for the run to settle and answer with its record.
    ///
    /// # Errors
    ///
    /// The run's own error, or a report that the executing task went away
    /// without producing one (a panic, or a runtime shutting down under it).
    pub async fn settled(
        self,
    ) -> Result<crate::workflows::RunRecord, crate::workflows::WorkflowError> {
        let run_id = self.run_id.clone();
        joined(run_id, self.join.await)
    }

    /// Wait up to `budget` for the run to settle.
    ///
    /// `Ok(None)` means the budget expired and the run is still going — not a
    /// failure, and deliberately not phrased as one. The run continues; the
    /// caller still holds its id.
    ///
    /// # Errors
    ///
    /// As [`settled`](Self::settled), for a run that did finish, badly.
    pub async fn settled_within(
        mut self,
        budget: std::time::Duration,
    ) -> Result<Option<crate::workflows::RunRecord>, crate::workflows::WorkflowError> {
        match tokio::time::timeout(budget, &mut self.join).await {
            Ok(result) => joined(self.run_id.clone(), result).map(Some),
            Err(_) => Ok(None),
        }
    }
}

/// Unwrap a join result, turning a task that vanished into a readable error.
fn joined(
    run_id: String,
    result: Result<
        Result<crate::workflows::RunRecord, crate::workflows::WorkflowError>,
        tokio::task::JoinError,
    >,
) -> Result<crate::workflows::RunRecord, crate::workflows::WorkflowError> {
    match result {
        Ok(outcome) => outcome,
        // The record itself is still readable — `RunFinalizer` reconciles it on
        // drop — so point at it rather than leaving the caller with nothing.
        Err(err) => Err(crate::workflows::WorkflowError::Engine(format!(
            "the task executing run '{run_id}' stopped without settling it ({err}); read the run \
             record for what it managed to do"
        ))),
    }
}

impl LocalRun<'_> {
    /// Start the run and return as soon as it is admitted.
    ///
    /// Everything that can be refused cheaply is refused *here*, before the
    /// task detaches: workflows turned off, an unknown or disabled workflow, a
    /// host that cannot start. A caller therefore learns about those the way it
    /// always did — as an error from the call — and only a run that genuinely
    /// began comes back as a run id.
    ///
    /// The `running` record is written before the task is spawned rather than
    /// by the task itself: the caller is handed this id and may poll it in its
    /// very next breath, and a task the runtime has not polled yet would answer
    /// that poll with `NotFound`.
    ///
    /// The host is moved into the task, so it lives exactly as long as the run
    /// and its loopback endpoints are unbound when the run ends.
    ///
    /// # Errors
    ///
    /// Fails when no coding-agent CLI is installed, when the workflow or the
    /// host is disabled, or when the run record cannot be written.
    pub async fn start(self) -> Result<StartedRun, crate::workflows::WorkflowError> {
        use crate::flow_engine::{folding_sink, CapabilitySettings, HostServices};
        use crate::workflows::{RunContext, StoreWorkflowResolver};

        let LocalRun {
            store,
            config,
            custom_harnesses,
            env,
            cwd,
            workflow_id,
            input,
            sink,
        } = self;

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
        let workflow = crate::workflows::store::require(store.as_ref(), workflow_id)?;
        if !workflow.enabled {
            return Err(crate::workflows::WorkflowError::Engine(format!(
                "workflow '{workflow_id}' is disabled"
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

        let sink = sink.unwrap_or_else(|| folding_sink().0);
        let max_loop_iterations = settings.max_loop_iterations;
        let context = RunContext {
            store: store.clone(),
            settings: Arc::new(settings),
            services: HostServices {
                dispatch: host.dispatch(),
                resolver: Arc::new(StoreWorkflowResolver::new(
                    store.clone(),
                    max_loop_iterations,
                )),
                http_credentials: std::collections::HashMap::new(),
            },
            sink,
        };

        let run_id = format!("run-{}", uuid::Uuid::new_v4());
        let started_at = crate::clock::now_millis() as u64;
        let admitted = crate::workflows::new_run_record(&run_id, workflow_id, started_at);
        store.record_run(&admitted)?;

        let workflow_id = workflow_id.to_string();
        let join = tokio::spawn({
            let run_id = run_id.clone();
            let workflow_id = workflow_id.clone();
            let store = store.clone();
            async move {
                // Held for the whole run and dropped with it, which unbinds the
                // loopback endpoints so a later run can bind them again.
                let _host = host;
                let outcome = crate::workflows::run_workflow(
                    context,
                    &workflow_id,
                    &run_id,
                    input.trigger,
                    input.inputs,
                )
                .await;
                // `run_workflow` can refuse the run (bad inputs, a disabled
                // workflow, a claim conflict) before it ever writes its own
                // record — the admission record above is all that exists for
                // this run id. Without this, that record is stuck at
                // `running` forever, since nothing else ever reconciles it.
                if let Err(err) = &outcome {
                    let mut record =
                        crate::workflows::new_run_record(&run_id, &workflow_id, started_at);
                    record.status = crate::workflows::RunStatus::Failed;
                    record.error = Some(err.to_string());
                    record.finished_at = Some(crate::clock::now_millis() as u64);
                    if let Err(store_err) = store.record_run(&record) {
                        tracing::warn!(
                            run = %run_id,
                            "could not finalize admitted run record: {store_err}"
                        );
                    }
                }
                outcome
            }
        });

        Ok(StartedRun {
            run_id,
            workflow_id,
            join,
        })
    }
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
/// The evolution counterpart to [`LocalRun`], and it starts the same embedded
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
    // Checked before the host starts, for the same reason `LocalRun::start` checks
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
