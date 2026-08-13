//! Assembling the capability bundle the engine runs against.
//!
//! There are exactly two entry points out of this module —
//! [`build_capabilities`] and [`open_checkpointer`] — and everything downstream
//! goes through them. Keeping the surface that narrow is what makes a re-vendor
//! of the engine a one-file problem: when its `Capabilities` struct gains a
//! field, this is the only place that has to learn about it.
//!
//! Each capability lives in its own submodule beside this one, named for the
//! engine trait it satisfies.
//!
//! Only the ones that are *about Medulla* are still written here: dispatching a
//! node to a harness, choosing which harness, and the `medulla:` tool namespace.
//! The rest — running a script out of process, keying state onto disk, refusing
//! an outbound URL that resolves into a private range — had no Medulla in them,
//! and now live in [`tinyflows::caps::host`] where the other hosts can have
//! them too. They are re-exported below so a call site here still names one
//! place.

pub mod agent;
pub mod dispatch;
pub mod tasks;
pub mod tools;

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tinyflows::caps::host::{
    AllowlistHttpClient, DeniedCodeRunner, FileStateStore, HostAllowlist, ProcessCodeRunner,
    ProcessShellRunner, ScriptPolicy,
};
use tinyflows::caps::{Capabilities, WorkflowResolver};
use tinyflows::engine::{Checkpointer, FileCheckpointer};

use crate::flow_engine::agent_evidence::AgentEvidence;
use crate::flow_engine::observability::NodeProgressSink;
use crate::flow_engine::settings::CapabilitySettings;

use self::agent::{HarnessAgentRunner, HarnessLlm};
use self::dispatch::HarnessDispatch;
use self::tasks::{MedullaTaskRunner, TaskCapabilities};
use self::tools::MedullaToolInvoker;

/// Schema-aware capability stand-ins for dry runs, now owned by the engine
/// crate.
pub use tinyflows::caps::host::mocks;
/// The out-of-process script runner and its calling convention, now owned by
/// the engine crate.
pub use tinyflows::caps::host::script;
/// Which files a script step may read and run in, now owned by the engine
/// crate.
pub use tinyflows::caps::host::script_policy;
/// The HTTP credential a `connection_ref` names.
pub use tinyflows::caps::host::{http_cred_name, HttpCredential, HTTP_CRED_PREFIX};

/// Everything a run needs from the host, other than its settings.
///
/// Grouped into one struct because [`build_capabilities`] would otherwise take
/// six positional arguments of which three are `Arc<dyn …>`, and a caller would
/// eventually pass them in the wrong order.
pub struct HostServices {
    /// Where `agent` nodes send their work.
    pub dispatch: Arc<dyn HarnessDispatch>,
    /// How `sub_workflow` nodes find their child graph.
    pub resolver: Arc<dyn WorkflowResolver>,
    /// HTTP credentials, keyed by the name a `connection_ref` uses.
    pub http_credentials: HashMap<String, HttpCredential>,
    /// Where a dispatched harness's live progress goes, when anyone is
    /// watching. `None` costs the run nothing: with no sink the dispatch asks
    /// for no status channel at all, so a headless run does not pay to
    /// assemble frames nobody reads.
    pub node_progress: Option<NodeProgressSink>,
}

impl HostServices {
    /// The services a run needs with nothing watching its harnesses.
    pub fn new(
        dispatch: Arc<dyn HarnessDispatch>,
        resolver: Arc<dyn WorkflowResolver>,
        http_credentials: HashMap<String, HttpCredential>,
    ) -> Self {
        Self {
            dispatch,
            resolver,
            http_credentials,
            node_progress: None,
        }
    }

    /// Stream every `agent` node's harness progress into `sink`.
    #[must_use]
    pub fn watching(mut self, sink: NodeProgressSink) -> Self {
        self.node_progress = Some(sink);
        self
    }
}

/// Build the capability bundle for one run.
///
/// `state_namespace` scopes the [`StateStore`](tinyflows::caps::StateStore) —
/// conventionally `workflow:<id>`, so two workflows never collide on a key.
/// `run_id` tags every dispatched task, and is the id an abort matches.
pub fn build_capabilities(
    settings: Arc<CapabilitySettings>,
    services: HostServices,
    state_namespace: &str,
    run_id: &str,
) -> Capabilities {
    build_capabilities_inner(settings, services, state_namespace, run_id, None)
}

/// Build capabilities that also capture resolved agent prompts for run history.
pub(crate) fn build_capabilities_with_agent_evidence(
    settings: Arc<CapabilitySettings>,
    services: HostServices,
    state_namespace: &str,
    run_id: &str,
    evidence: Arc<AgentEvidence>,
) -> Capabilities {
    build_capabilities_inner(settings, services, state_namespace, run_id, Some(evidence))
}

/// Shared capability assembly, optionally instrumented with agent evidence.
fn build_capabilities_inner(
    settings: Arc<CapabilitySettings>,
    services: HostServices,
    state_namespace: &str,
    run_id: &str,
    evidence: Option<Arc<AgentEvidence>>,
) -> Capabilities {
    // One limiter and one id sequence for the whole run, minted here rather
    // than inside `Assembly::build` so every bundle the assembly produces — the
    // run's own and each spawned task's — shares them. Two counters each
    // starting at zero would hand the same task id to the first dispatch each
    // makes along a shared route, and two semaphores would make the run's real
    // ceiling twice what the operator configured.
    let slots = Arc::new(tokio::sync::Semaphore::new(
        settings.max_parallel_agents.max(1),
    ));
    let sequence = Arc::new(AtomicU64::new(0));

    Assembly {
        settings,
        dispatch: services.dispatch,
        resolver: services.resolver,
        http_credentials: services.http_credentials,
        node_progress: services.node_progress,
        state_namespace: state_namespace.to_string(),
        run_id: run_id.to_string(),
        evidence,
        slots,
        sequence,
    }
    .build()
}

/// Everything one run's capability bundle is assembled from.
///
/// A struct rather than a function's locals because the bundle has to be
/// buildable more than once: a `spawn` node's task runner rebuilds it for each
/// task it starts, since a bundle cannot hold the runner that holds the bundle
/// without leaking the pair. Cloning one is cheap — every field is an `Arc`, a
/// short string, or a small map.
#[derive(Clone)]
struct Assembly {
    /// The operator's limits and defaults for this run.
    settings: Arc<CapabilitySettings>,
    /// Where `agent` nodes send their work.
    dispatch: Arc<dyn HarnessDispatch>,
    /// How `sub_workflow` nodes find their child graph.
    resolver: Arc<dyn WorkflowResolver>,
    /// HTTP credentials, keyed by the name a `connection_ref` uses.
    http_credentials: HashMap<String, HttpCredential>,
    /// Where a dispatched harness's live progress goes, if anyone is watching.
    node_progress: Option<NodeProgressSink>,
    /// Scopes the state store, so two workflows never collide on a key.
    state_namespace: String,
    /// Tags every dispatched task; the id an abort matches.
    run_id: String,
    /// Captures resolved agent prompts for run history, when recording.
    evidence: Option<Arc<AgentEvidence>>,
    /// The run-wide ceiling on harness tasks in flight.
    slots: Arc<tokio::sync::Semaphore>,
    /// The run-wide dispatch id sequence.
    sequence: Arc<AtomicU64>,
}

impl Assembly {
    /// Assemble one capability bundle.
    fn build(&self) -> Capabilities {
        let settings = self.settings.clone();
        let code: Arc<dyn tinyflows::caps::CodeRunner> = if settings.allow_code {
        // The same bound `medulla:shell` uses, so an author who moves a
        // script between the two does not silently change its deadline.
            Arc::new(ProcessCodeRunner::new(settings.script_timeout()))
        } else {
            Arc::new(DeniedCodeRunner)
        };

        let slots = self.slots.clone();
        let sequence = self.sequence.clone();
        let run_id = self.run_id.as_str();
        let progress = self.node_progress.clone();
        let llm: Arc<dyn tinyflows::caps::LlmProvider> = match &self.evidence {
            Some(evidence) => Arc::new(
                HarnessLlm::recording(
                    self.dispatch.clone(),
                    settings.clone(),
                    run_id,
                    evidence.clone(),
                )
                .with_limiter(slots.clone())
                .with_sequence(sequence.clone())
                .streaming_to(progress.clone()),
            ),
            None => Arc::new(
                HarnessLlm::new(self.dispatch.clone(), settings.clone(), run_id)
                    .with_limiter(slots.clone())
                    .with_sequence(sequence.clone())
                    .streaming_to(progress.clone()),
            ),
        };
        let agent: Arc<dyn tinyflows::caps::AgentRunner> = match &self.evidence {
            Some(evidence) => Arc::new(
                HarnessAgentRunner::recording(
                    self.dispatch.clone(),
                    settings.clone(),
                    run_id,
                    evidence.clone(),
                )
                .with_limiter(slots)
                .with_sequence(sequence)
                .streaming_to(progress),
            ),
            None => Arc::new(
                HarnessAgentRunner::new(self.dispatch.clone(), settings.clone(), run_id)
                    .with_limiter(slots)
                    .with_sequence(sequence)
                    .streaming_to(progress),
            ),
        };

        // A `shell` node is offered exactly when a `code` node is: both run an
        // author's script with this daemon's privileges, so a host that refused
        // one and allowed the other would be drawing a line that does not
        // exist. `None` rather than a refusing runner, because the engine's own
        // answer for an absent capability already says the node cannot run here.
        let shell: Option<Arc<dyn tinyflows::caps::ShellRunner>> = settings.allow_code.then(|| {
            Arc::new(ProcessShellRunner::new(
                ScriptPolicy::new(&settings.workspace),
                settings.script_timeout(),
            )) as Arc<dyn tinyflows::caps::ShellRunner>
        });

        Capabilities {
            llm,
            agent: Some(agent),
            shell,
            tools: Arc::new(MedullaToolInvoker::new(settings.clone())),
            http: Arc::new(AllowlistHttpClient::new(
                HostAllowlist::new(settings.http_allowlist.clone()),
                self.http_credentials.clone(),
            )),
            code,
            state: Arc::new(FileStateStore::new(
                &settings.state_dir,
                &self.state_namespace,
            )),
            resolver: self.resolver.clone(),
            // `None` until the host exposes a memory store: the engine then
            // fails a `memory` node with a capability error, which is the honest
            // answer. Standing one up on the state store would answer `recall`
            // with an empty set — indistinguishable from "the user never said
            // that".
            memory: None,
            // The engine's default runner schedules on tokio but owns no
            // capabilities, so it echoes a spawn's spec back instead of
            // performing it — a `spawn` would look like it ran and produce
            // nothing usable. This one performs the work against this run's own
            // capabilities, which is what makes `spawn`/`gate` fan-out real here
            // rather than a shape the graph can express and the host cannot
            // honour.
            tasks: Some(Arc::new(MedullaTaskRunner::new(Arc::new(self.clone())))),
        }
    }
}

impl TaskCapabilities for Assembly {
    fn capabilities(&self) -> Capabilities {
        self.build()
    }

    fn timeout(&self) -> Duration {
        // The same bound the run itself gets. A spawned task is work the run
        // asked for, so it has no business outliving the run's own deadline.
        Duration::from_secs(self.settings.run_timeout_secs)
    }
}

/// Build a capability bundle that touches nothing outside the process.
///
/// Used for validating a graph by simulation: expressions resolve against real
/// values, node schemas are satisfied, and no harness session is started. The
/// tool invoker is the engine's mock behind [`tools::PreflightToolInvoker`], so
/// an argument that failed to resolve is still caught.
pub fn build_dry_run_capabilities(resolver: Arc<dyn WorkflowResolver>) -> Capabilities {
    let mut caps = tinyflows::caps::mock::mock_capabilities();
    caps.llm = Arc::new(mocks::SchemaAwareMockLlm);
    caps.agent = Some(Arc::new(mocks::SchemaAwareMockAgentRunner));
    caps.tools = Arc::new(tools::PreflightToolInvoker::new(caps.tools.clone()));
    caps.resolver = resolver;
    // Deliberately no task runner, which makes a `spawn` node run its work
    // inline against these same stand-ins. That is what a dry run is for: the
    // engine's default runner would settle the ticket with an echo of the spec,
    // so a child graph that does not compile, or an argument that resolved to
    // null, would sail through the very check meant to catch it.
    caps.tasks = None;
    caps
}

/// Open the run checkpointer.
///
/// A checkpoint is what makes an approval pause outlive the process: a run that
/// parked yesterday is resumed by handing the engine the same `thread_id`. The
/// engine's own file-backed implementation is reused rather than written afresh
/// — a bespoke one would only be a second thing to get wrong.
pub fn open_checkpointer(settings: &CapabilitySettings) -> Arc<dyn Checkpointer<Value>> {
    Arc::new(FileCheckpointer::<Value>::new(&settings.checkpoint_dir))
}
