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
pub mod tools;

use std::collections::HashMap;
use std::sync::Arc;

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
    let code: Arc<dyn tinyflows::caps::CodeRunner> = if settings.allow_code {
        // The same bound `medulla:shell` uses, so an author who moves a
        // script between the two does not silently change its deadline.
        Arc::new(ProcessCodeRunner::new(settings.script_timeout()))
    } else {
        Arc::new(DeniedCodeRunner)
    };

    // One limiter for the whole run. The agent runner and the LLM provider both
    // dispatch to the same worker pool, so giving each its own semaphore would
    // make the run's real ceiling twice what the operator configured.
    let slots = Arc::new(tokio::sync::Semaphore::new(
        settings.max_parallel_agents.max(1),
    ));

    // One task-id sequence for the whole run, for the same reason as the
    // limiter: both runners mint `wf:{run}:{route}#{sequence}`, so two counters
    // each starting at zero would hand the same id to the first dispatch each
    // makes along a shared route.
    let sequence = Arc::new(std::sync::atomic::AtomicU64::new(0));

    let progress = services.node_progress.clone();
    let llm: Arc<dyn tinyflows::caps::LlmProvider> = match &evidence {
        Some(evidence) => Arc::new(
            HarnessLlm::recording(
                services.dispatch.clone(),
                settings.clone(),
                run_id,
                evidence.clone(),
            )
            .with_limiter(slots.clone())
            .with_sequence(sequence.clone())
            .streaming_to(progress.clone()),
        ),
        None => Arc::new(
            HarnessLlm::new(services.dispatch.clone(), settings.clone(), run_id)
                .with_limiter(slots.clone())
                .with_sequence(sequence.clone())
                .streaming_to(progress.clone()),
        ),
    };
    let agent: Arc<dyn tinyflows::caps::AgentRunner> = match evidence {
        Some(evidence) => Arc::new(
            HarnessAgentRunner::recording(services.dispatch, settings.clone(), run_id, evidence)
                .with_limiter(slots)
                .with_sequence(sequence)
                .streaming_to(progress),
        ),
        None => Arc::new(
            HarnessAgentRunner::new(services.dispatch, settings.clone(), run_id)
                .with_limiter(slots)
                .with_sequence(sequence)
                .streaming_to(progress),
        ),
    };

    // A `shell` node is offered exactly when a `code` node is: both run an
    // author's script with this daemon's privileges, so a host that refused one
    // and allowed the other would be drawing a line that does not exist.
    // `None` rather than a refusing runner, because the engine's own answer for
    // an absent capability already says the node cannot run here.
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
            services.http_credentials,
        )),
        code,
        state: Arc::new(FileStateStore::new(&settings.state_dir, state_namespace)),
        resolver: services.resolver,
        // `None` until the host exposes a memory store: the engine then fails a
        // `memory` node with a capability error, which is the honest answer.
        // Standing one up on the state store would answer `recall` with an empty
        // set — indistinguishable from "the user never said that".
        memory: None,
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
