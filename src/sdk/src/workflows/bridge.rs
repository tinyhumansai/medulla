//! This host's side of the cloud workflow plane: an adapter, and nothing else.
//!
//! The transport — the advert batch, the `medulla:workflow_request` round trip,
//! the deadlines the backend enforces — lives in the embedded OpenHuman core
//! ([`openhuman_core::openhuman::platform::socket::medulla::workflows`]), because two
//! hosts share it and only one of them is this crate. What the core cannot
//! supply is the *store*: it deliberately knows nothing about a graph, a node
//! kind, or a run record. So it asks for a [`WorkflowBridge`] and this module
//! supplies one, mapping each of its store-facing methods onto the operation
//! [`crate::workflows::ops`] already exposes to the CLI, the MCP tools and the
//! TUI.
//!
//! **Nothing here is new workflow behaviour, on purpose.** A read that answered
//! differently over the socket than it does over `medulla workflow get` would be
//! a second implementation of the catalogue, free to drift from the first. Every
//! method below is therefore a one-line delegation plus an error conversion, and
//! the only real code in the file is [`descriptor`] (the summary→advert
//! projection) and [`trigger_input`] (a task's text as a trigger payload).
//!
//! **The run path is here too.** A task frame naming a `workflow` runs that
//! saved graph instead of handing its text to a harness, with the *task id as
//! the run id* — which is what makes the orchestrator's ordinary abort cancel
//! the run, since every node the run dispatches carries that id as its
//! `abort_id`. That contract is `docs/workflows.md`'s, written for tiny.place
//! peers; [`run_task_workflow`] reuses it verbatim for the cloud plane rather
//! than inventing a second one.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use openhuman_core::openhuman::platform::socket::medulla::payloads::{
    CopilotOutcome as WireCopilotOutcome, WorkflowDescriptor, WorkflowInputDescriptor,
};
use openhuman_core::openhuman::platform::socket::medulla::workflows::WorkflowBridge;

use crate::flow_engine::caps::dispatch::HarnessDispatch;
use crate::protocol::HarnessProvider;
use crate::workflows::{
    ops, run_workflow, CopilotSession, RunContext, RunRecord, WorkflowError, WorkflowStore,
    WorkflowSummary,
};

/// The store side of the cloud workflow plane, backed by this host's layered
/// JSON workflow store.
///
/// Install it once with
/// [`set_workflow_bridge`](openhuman_core::openhuman::platform::socket::medulla::workflows::set_workflow_bridge)
/// and re-advertise with
/// [`emit_register_workflows`](openhuman_core::openhuman::platform::socket::medulla::workflows::emit_register_workflows)
/// whenever the store changes.
///
/// **Install it only when workflows are enabled on this host.** Nothing in here
/// consults `workflows.enabled`: the bridge is a view of the store, and the
/// refusal belongs to [`run_workflow`], which already enforces it. Advertising
/// graphs this host would refuse to run only teaches an orchestrator to
/// delegate work that will bounce, so a host with workflows off should not
/// install a bridge at all (or should retract with `clear_workflow_bridge`).
pub struct StoreWorkflowBridge {
    /// The store every read goes to. The same handle the CLI and the MCP tools
    /// use, so all three answer identically.
    store: Arc<dyn WorkflowStore>,
    /// Where a copilot turn runs. `None` disables authoring: the op reports that
    /// rather than pretending to have tried, because a "copilot" whose reply is
    /// silence costs the backend its ten-minute deadline.
    copilot: Option<CopilotDispatch>,
    /// Provenance stamped on the advert batch when this host runs as a named
    /// roster agent.
    agent_id: Option<String>,
    /// The directory this host works in, reported on capability probes.
    action_dir: Option<String>,
}

/// Everything a copilot turn needs beyond the store.
///
/// Held apart from [`CopilotSession`] because a session is built per turn (it
/// borrows the store by `Arc` clone), while these settings are fixed for the
/// life of the bridge.
struct CopilotDispatch {
    /// Where the authoring turn is run.
    dispatch: Arc<dyn HarnessDispatch>,
    /// The bridge address of the worker that runs it.
    worker_address: String,
    /// Optional harness hint.
    provider: Option<HarnessProvider>,
    /// Optional model hint.
    model: Option<String>,
}

impl StoreWorkflowBridge {
    /// A read-only bridge over `store`.
    ///
    /// `copilot` requests are refused until [`Self::with_copilot`] supplies a
    /// dispatch; the four reads work immediately.
    pub fn new(store: Arc<dyn WorkflowStore>) -> Self {
        Self {
            store,
            copilot: None,
            agent_id: None,
            action_dir: None,
        }
    }

    /// Enable the authoring turn, running it through `dispatch` against the
    /// worker at `worker_address`.
    ///
    /// `provider` and `model` are hints forwarded to the harness, exactly as the
    /// TUI's own copilot pane forwards them.
    pub fn with_copilot(
        mut self,
        dispatch: Arc<dyn HarnessDispatch>,
        worker_address: impl Into<String>,
        provider: Option<HarnessProvider>,
        model: Option<String>,
    ) -> Self {
        self.copilot = Some(CopilotDispatch {
            dispatch,
            worker_address: worker_address.into(),
            provider,
            model,
        });
        self
    }

    /// Name the roster agent that owns these workflows.
    ///
    /// Left unset the backend treats the whole socket as the owner, which is
    /// right for a host running one identity — the usual case.
    pub fn with_agent_id(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = Some(agent_id.into());
        self
    }

    /// Name the directory this host reads and writes in.
    ///
    /// The transport asks the bridge for this rather than reloading the core's
    /// own config, so a host that does not supply it reports no `cwd` at all on
    /// a capability probe — and `cwd` is how the orchestrator answers "where
    /// does this agent work" when it places a task. Left unset the field is
    /// omitted rather than guessed.
    pub fn with_action_dir(mut self, action_dir: impl Into<String>) -> Self {
        self.action_dir = Some(action_dir.into());
        self
    }
}

#[async_trait]
impl WorkflowBridge for StoreWorkflowBridge {
    /// Every installed workflow as an advert.
    ///
    /// An unreadable store advertises nothing rather than failing, for the
    /// callers that cannot act on the difference: a host whose workflow
    /// directory is missing is still a perfectly good host for ordinary tasks.
    /// Registration takes [`Self::try_list`] instead, which can.
    fn list(&self) -> Vec<WorkflowDescriptor> {
        self.try_list().unwrap_or_default()
    }

    /// The same read, with the store's failure kept.
    ///
    /// Overridden because a registration batch is a *replacement*: the backend
    /// drops whatever it held for this socket and takes what arrives. An empty
    /// batch therefore says "this host has no workflows", which is a very
    /// different claim from "this host could not read its store just now" — and
    /// the second one must not retract graphs that are still installed. The
    /// transport skips the emission entirely when this fails.
    fn try_list(&self) -> Result<Vec<WorkflowDescriptor>, String> {
        Ok(self
            .store
            .list()
            .map_err(readable)?
            .iter()
            .map(descriptor)
            .collect())
    }

    fn get(&self, id: &str) -> Result<Value, String> {
        ops::get(&self.store, id).map_err(readable)
    }

    fn node_kinds(&self, kind: Option<&str>) -> Result<Value, String> {
        ops::catalog(kind).map_err(readable)
    }

    fn runs(&self, id: &str) -> Result<Value, String> {
        ops::list_runs(&self.store, id).map_err(readable)
    }

    /// One authoring turn on this host's own copilot.
    ///
    /// Naming a workflow edits it ([`CopilotSession::turn`]); naming none
    /// creates one ([`CopilotSession::create`]). Either way the outcome is
    /// [`crate::workflows::CopilotOutcome`] *verbatim* — it is already derived
    /// from a re-read of the store rather than from the model's claim, and that
    /// is the accountability property the whole delegation rests on. Re-deriving
    /// anything here would weaken it.
    ///
    /// No status channel is passed: the backend correlates one request to one
    /// result and has nowhere to put progress frames.
    async fn copilot(
        &self,
        instruction: &str,
        workflow_id: Option<&str>,
    ) -> Result<WireCopilotOutcome, String> {
        let Some(copilot) = self.copilot.as_ref() else {
            return Err("this host has no authoring copilot configured".to_string());
        };
        let session = CopilotSession {
            store: self.store.clone(),
            dispatch: copilot.dispatch.clone(),
            worker_address: copilot.worker_address.clone(),
            provider: copilot.provider,
            model: copilot.model.clone(),
            // One conversation per workflow, exactly as the TUI keys one per
            // open pane. Successive requests about the same graph are a
            // conversation to the orchestrator too — "now do the same to the
            // other node" is as ordinary over the socket as it is in the pane —
            // and keying by id keeps two workflows being edited in parallel from
            // reading each other's turns. Create turns share one key because
            // there is no id to key on until the turn succeeds.
            conversation: match workflow_id {
                Some(id) => format!("cloud-copilot:{id}"),
                None => "cloud-copilot:new".to_string(),
            },
            // The saved transcripts belong to the operator's own panes. A
            // request arriving over the socket is a different conversation
            // with a different party, and replaying one into the other would
            // hand the orchestrator a context nobody on this end offered it.
            recap: None,
        };
        let outcome = match workflow_id {
            Some(id) => session.turn(id, instruction, None).await,
            None => session.create(instruction, None).await,
        }
        .map_err(readable)?;

        // `CopilotOutcome::removed` has no field on the wire, and needs none:
        // the copilot already records a deletion as a `− workflow …` line in
        // `changes`, so the orchestrator is told. The flag exists for callers
        // holding resources scoped to the workflow (the TUI's pane), and this
        // adapter holds none.
        Ok(WireCopilotOutcome {
            reply: outcome.reply,
            changes: outcome.changes,
            created: outcome.created,
        })
    }

    fn agent_id(&self) -> Option<String> {
        self.agent_id.clone()
    }

    fn action_dir(&self) -> Option<String> {
        self.action_dir.clone()
    }
}

/// One stored workflow as the advert the orchestrator reads.
///
/// `node_count` saturates rather than wrapping: the wire field is a `u32` and a
/// graph large enough to overflow it is already beyond anything the count is
/// meant to convey (a rough cost signal), so a clamped maximum reads correctly
/// where a wrapped tiny number would read as cheap.
fn descriptor(summary: &WorkflowSummary) -> WorkflowDescriptor {
    WorkflowDescriptor {
        id: summary.id.clone(),
        name: summary.name.clone(),
        description: summary.description.clone(),
        node_count: u32::try_from(summary.node_count).unwrap_or(u32::MAX),
        enabled: Some(summary.enabled),
        trigger_kind: summary.trigger_kind.clone(),
        // Provenance is the batch's, not the workflow's: this host stamps one
        // `agentId` on the whole registration and the backend fans it out.
        agent_id: None,
        workspace_id: None,
        // The declared inputs travel with the advert so the orchestrator knows
        // what running this costs it in arguments before it picks the workflow,
        // rather than discovering it by being refused.
        inputs: summary
            .inputs
            .iter()
            .map(|input| WorkflowInputDescriptor {
                name: input.name.clone(),
                ty: input.ty.as_str().to_string(),
                description: input.description.clone().unwrap_or_default(),
                required: input.required,
                default: input.default.clone(),
            })
            .collect(),
    }
}

/// A workflow error as the sentence a model will read.
///
/// The transport puts this string straight onto a tool result, so it has to be
/// actionable on its own — which [`WorkflowError`]'s `Display` already is (it
/// names the id, and lists every validation failure rather than the first).
fn readable(error: WorkflowError) -> String {
    error.to_string()
}

/// Run the workflow a task frame named, with the frame's task id as the run id.
///
/// That identity is the whole contract: the run id is the engine checkpointer's
/// thread id *and* the `abort_id` on every task the run dispatches, so the
/// orchestrator aborting the task it sent cancels the graph and the harness
/// session it is waiting on, with no workflow-specific abort path.
///
/// # Errors
///
/// Fails when workflows are disabled on this host, when the workflow is unknown
/// or disabled, when the graph does not compile, or when a run with this id is
/// already executing — a resent frame must not start the graph twice, because
/// both copies would run every node's side effects and race to overwrite the
/// same run record.
pub async fn run_task_workflow(
    context: RunContext,
    workflow_id: &str,
    task_id: &str,
    instruction: &str,
) -> Result<RunRecord, WorkflowError> {
    // No declared inputs: an orchestrator frame carries one instruction, not a
    // named argument set. A workflow that declares a required input therefore
    // refuses a task frame, with an error naming the input — which is the
    // honest answer until the frame protocol carries them.
    run_workflow(
        context,
        workflow_id,
        task_id,
        trigger_input(instruction),
        serde_json::Map::new(),
    )
    .await
}

/// Cancel the run started for `task_id`, reporting whether one was cancelled.
///
/// **Process-local**, and honestly so: the run registry lives in this process,
/// so this cancels a run *this* host started and nothing else. That is not a
/// limitation on the path that matters — an abort frame arrives at the host
/// executing the run, which is the one holding the registry — but a caller
/// relaying an abort on someone else's behalf will get `false` and must not read
/// it as "already finished".
pub fn cancel_task_workflow(task_id: &str) -> bool {
    crate::workflows::run::cancel(task_id)
}

/// The trigger payload for a run, from a task frame's text.
///
/// JSON when the sender sent JSON, and `{ "text": … }` otherwise — so a frame
/// carrying an ordinary instruction still gives the graph's expressions
/// something to read. Empty text becomes an empty object rather than
/// `{"text":""}`, which would look like a deliberate blank input.
pub fn trigger_input(text: &str) -> Value {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Value::Object(Default::default());
    }
    serde_json::from_str(trimmed).unwrap_or_else(|_| serde_json::json!({ "text": text }))
}

#[cfg(test)]
#[path = "bridge_tests.rs"]
mod tests;
