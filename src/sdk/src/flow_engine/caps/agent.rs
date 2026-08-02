//! `agent` nodes, run on a real harness.
//!
//! This is where Medulla's integration differs from every other host embedding
//! this engine. Elsewhere an `agent` node is a model call, or an in-process
//! agent loop. Here it is a *task dispatched to a coding harness* — Claude Code,
//! Codex, or OpenCode, local or on another machine — over the same bridge the
//! orchestrator already uses. A workflow is therefore a plan whose steps are
//! real harness sessions, not a chain of completions.
//!
//! Two routes, chosen by [`route_for_agent_ref`]:
//!
//! - **Template** — `agent_ref` names an agent template, whose id becomes the
//!   worker address the task is sent to.
//! - **Default** — no `agent_ref`, so the node runs on the host's default
//!   worker.
//!
//! *Which* harness and model, as distinct from which machine, is a separate
//! choice: `config.harness` and `config.model` on the node, falling back to the
//! workflow's `defaults` and then to the host's config. See
//! [`crate::flow_engine::harness_choice`] for how those layers combine.
//!
//! `agent_ref` — and, for the same reason, `harness` — is read from the node's
//! *config*, never from model output. That is the engine's own guard and it
//! matters more here than upstream: a prompt injection that could choose the
//! `agent_ref` would be choosing which machine runs the next instruction, and
//! one that could choose the harness would be choosing which binary and which
//! credentials run it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Semaphore;

use async_trait::async_trait;
use serde_json::{json, Value};
use tinyflows::caps::{AgentRunner, LlmProvider};
use tinyflows::error::{EngineError, Result};

use crate::flow_engine::agent_evidence::AgentEvidence;
use crate::flow_engine::harness_choice::{HarnessChoice, HarnessPreference};
use crate::flow_engine::settings::CapabilitySettings;
use crate::hub::{RunError, TaskRequest};

use super::dispatch::HarnessDispatch;

/// Where an `agent` node's work should go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentRoute {
    /// A named template, dispatched to the worker of that name.
    Template(String),
    /// The host's configured default worker.
    Default,
}

/// Choose a route for an `agent_ref`.
///
/// An empty or absent reference is the default route rather than an error: a
/// graph that just says "run this prompt" is the common case and should not
/// require an operator to name a worker in every node.
pub fn route_for_agent_ref(agent_ref: Option<&str>) -> AgentRoute {
    match agent_ref.map(str::trim).filter(|r| !r.is_empty()) {
        Some(reference) => AgentRoute::Template(reference.to_string()),
        None => AgentRoute::Default,
    }
}

/// The instruction text an `agent` node carries.
///
/// `prompt` is the engine's own field name for it; `instruction` is accepted
/// because that is what the rest of Medulla calls the same thing, and an author
/// moving between the two surfaces should not be caught out.
pub fn instruction_of(request: &Value) -> Result<String> {
    for key in ["prompt", "instruction", "input"] {
        if let Some(text) = request.get(key).and_then(Value::as_str) {
            let text = text.trim();
            if !text.is_empty() {
                return Ok(text.to_string());
            }
        }
    }
    Err(EngineError::Capability(
        "agent node: no prompt — set `prompt` in the node config".to_string(),
    ))
}

/// Shape a harness reply into the value the `agent` node expects.
///
/// The engine wraps this in its `{ json, text, raw }` envelope and reads `text`
/// out of a `text` field, so putting the reply there is what makes
/// `=item.text` work downstream. A reply that happens to be JSON is *also*
/// surfaced structurally, so `=item.json.…` works without a parser node.
pub fn reply_to_value(reply: &str, worker: &str) -> Value {
    let structured: Option<Value> = serde_json::from_str(reply.trim()).ok();
    json!({
        "text": reply,
        "json": structured,
        "worker": worker,
    })
}

/// Map a dispatch failure onto a capability error.
///
/// An abort keeps its identity in the message because it is the one failure a
/// retry must not paper over: the orchestrator cancelled the work deliberately.
fn dispatch_error(node_kind: &str, err: RunError) -> EngineError {
    EngineError::Capability(format!("{node_kind}: {err}"))
}

/// An [`AgentRunner`] that dispatches each node to a harness.
pub struct HarnessAgentRunner {
    dispatch: Arc<dyn HarnessDispatch>,
    settings: Arc<CapabilitySettings>,
    /// The run this belongs to, used to build ids a `task_abort` can match.
    run_id: String,
    /// Distinguishes one dispatch from the next within this run.
    ///
    /// Two parallel nodes routed to the same worker — the common case, since
    /// both may omit `agent_ref` — would otherwise share a wire id, and a
    /// worker dedupes on `sender + taskId`. One of the two would be rejected as
    /// a duplicate of the other.
    sequence: AtomicU64,
    /// Caps how many harness tasks this run has in flight at once.
    ///
    /// A per-item node fans out into one dispatch per item, and on this host a
    /// dispatch is a whole harness session. The engine bounds one node's width;
    /// this bounds the run, since several nodes can fan out at the same time.
    /// Shared across every runner built for the run (see
    /// [`with_limiter`](Self::with_limiter)), so the ceiling is a property of
    /// the run rather than of one node.
    slots: Arc<Semaphore>,
    /// Resolved prompts captured for the durable run inspector.
    evidence: Option<Arc<AgentEvidence>>,
}

impl HarnessAgentRunner {
    /// A runner dispatching through `dispatch` under `settings`, tagging every
    /// task with `run_id`.
    pub fn new(
        dispatch: Arc<dyn HarnessDispatch>,
        settings: Arc<CapabilitySettings>,
        run_id: impl Into<String>,
    ) -> Self {
        let slots = Arc::new(Semaphore::new(settings.max_parallel_agents.max(1)));
        Self {
            dispatch,
            settings,
            run_id: run_id.into(),
            sequence: AtomicU64::new(0),
            slots,
            evidence: None,
        }
    }

    /// Build a runner that records resolved prompts for run inspection.
    pub(crate) fn recording(
        dispatch: Arc<dyn HarnessDispatch>,
        settings: Arc<CapabilitySettings>,
        run_id: impl Into<String>,
        evidence: Arc<AgentEvidence>,
    ) -> Self {
        let settings_max_parallel = settings.max_parallel_agents.max(1);
        Self {
            dispatch,
            settings,
            run_id: run_id.into(),
            sequence: AtomicU64::new(0),
            slots: Arc::new(Semaphore::new(settings_max_parallel)),
            evidence: Some(evidence),
        }
    }

    /// Share an existing limiter instead of this runner's own.
    ///
    /// The run builds an agent runner *and* an LLM provider (which wraps a
    /// second runner), and both dispatch to the same worker pool. Left to
    /// themselves they would hold independent semaphores and the run's real
    /// ceiling would be double what the operator configured, so the caller
    /// builds one limiter and hands it to both.
    #[must_use]
    pub(crate) fn with_limiter(mut self, slots: Arc<Semaphore>) -> Self {
        self.slots = slots;
        self
    }

    /// Capture a resolved prompt when this request came from a tagged agent node.
    fn record_prompt(&self, request: &Value, instruction: &str) {
        if let Some(evidence) = &self.evidence {
            evidence.record(request, instruction);
        }
    }

    /// Build the task frame for one node.
    ///
    /// The task id carries the run and the route so a worker-side log, and an
    /// operator reading it, can tell which workflow step a session belongs to.
    ///
    /// `choice` is the harness and model this node resolved to; see
    /// [`crate::flow_engine::harness_choice`] for how the layers combine.
    fn request(
        &self,
        route: &AgentRoute,
        instruction: String,
        choice: HarnessChoice,
    ) -> TaskRequest {
        let worker_address = match route {
            AgentRoute::Template(name) => name.clone(),
            AgentRoute::Default => self.settings.default_worker_address.clone(),
        };
        let suffix = match route {
            AgentRoute::Template(name) => name.clone(),
            AgentRoute::Default => "default".to_string(),
        };
        // The sequence is what keeps two concurrent nodes on one worker
        // distinguishable; the route name is there so a worker-side log says
        // which step it is looking at.
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let task_id = format!("wf:{}:{suffix}#{sequence}", self.run_id);
        TaskRequest {
            // Distinct ids on purpose: the worker dedupes on the wire id, while
            // the orchestrator aborts by the run id — so aborting the run
            // cancels whichever node is in flight.
            task_id,
            abort_id: self.run_id.clone(),
            cycle_id: Some(self.run_id.clone()),
            instruction,
            worker_address,
            provider: choice.provider,
            custom_harness: choice.custom_harness,
            model: choice.model,
            // No conversation, deliberately. Each node is its own unit of work,
            // and letting two share a harness session would make a graph's
            // behaviour depend on the order its branches happened to run in —
            // the same reason a task frame routes `Bounded` by default.
            conversation: None,
            // A node dispatches an instruction, never another workflow: nesting
            // is expressed with a `sub_workflow` node, which the engine expands
            // itself and applies its own depth limit to.
            tool_mode: None,
            workflow: None,
        }
    }

    /// Resolve which harness and model this node's config asks for.
    ///
    /// The node's own preference sits over the run's standing one. A config that
    /// names a harness this host cannot make sense of fails the node rather than
    /// falling back to the default: an author who wrote `harness: cluade` meant
    /// to change where the work runs, and silently running it somewhere else is
    /// the one outcome that helps nobody.
    ///
    /// # Errors
    ///
    /// Returns a capability error naming the node kind and the correction.
    fn choice_for(&self, node_kind: &str, config: &Value) -> Result<HarnessChoice> {
        let node = HarnessPreference::from_config(config)
            .map_err(|err| EngineError::Capability(format!("{node_kind}: {err}")))?;
        Ok(HarnessChoice::resolve(&[
            node,
            self.settings.harness_preference(),
        ]))
    }

    /// Dispatch `instruction` along `route` and shape the reply.
    async fn run_on_harness(
        &self,
        route: AgentRoute,
        instruction: String,
        choice: HarnessChoice,
    ) -> Result<Value> {
        let request = self.request(&route, instruction, choice);
        if request.worker_address.trim().is_empty() {
            return Err(EngineError::Capability(
                "agent node: no worker to dispatch to — set an `agent_ref` on the node or a \
                 default worker in the workflows config"
                    .to_string(),
            ));
        }
        // Wait for a slot before dispatching. A fanned-out node can reach here
        // once per input item simultaneously, and each dispatch occupies a
        // harness session for the length of a coding task. Waiting throttles an
        // over-wide fan-out; it never fails it.
        let _permit = self.slots.acquire().await.map_err(|_| {
            EngineError::Capability("agent node: run concurrency limiter closed".to_string())
        })?;
        let worker = request.worker_address.clone();
        let outcome = self
            .dispatch
            .dispatch(request)
            .await
            .map_err(|err| dispatch_error("agent node", err))?;
        Ok(reply_to_value(&outcome.reply, &worker))
    }
}

#[async_trait]
impl AgentRunner for HarnessAgentRunner {
    async fn run_agent(
        &self,
        agent_ref: &str,
        request: Value,
        _conn: Option<&str>,
    ) -> Result<Value> {
        let instruction = instruction_of(&request)?;
        self.record_prompt(&request, &instruction);
        let choice = self.choice_for("agent node", &request)?;
        self.run_on_harness(route_for_agent_ref(Some(agent_ref)), instruction, choice)
            .await
    }
}

/// An [`LlmProvider`] that also dispatches to a harness.
///
/// The engine falls back to this when a node names no `agent_ref`, and uses it
/// for `output_parser` repair passes. Both are "run this text somewhere and give
/// me the answer", which on this host means the default worker — Medulla has no
/// bare model client of its own, and inventing one would give a workflow a
/// second, ungoverned way to reach a provider.
pub struct HarnessLlm {
    inner: HarnessAgentRunner,
}

impl HarnessLlm {
    /// A provider over the same dispatch the agent runner uses.
    pub fn new(
        dispatch: Arc<dyn HarnessDispatch>,
        settings: Arc<CapabilitySettings>,
        run_id: impl Into<String>,
    ) -> Self {
        Self {
            inner: HarnessAgentRunner::new(dispatch, settings, run_id),
        }
    }

    /// A provider that records resolved agent prompts for run inspection.
    pub(crate) fn recording(
        dispatch: Arc<dyn HarnessDispatch>,
        settings: Arc<CapabilitySettings>,
        run_id: impl Into<String>,
        evidence: Arc<AgentEvidence>,
    ) -> Self {
        Self {
            inner: HarnessAgentRunner::recording(dispatch, settings, run_id, evidence),
        }
    }

    /// Share the run's dispatch limiter — see
    /// [`HarnessAgentRunner::with_limiter`].
    #[must_use]
    pub(crate) fn with_limiter(mut self, slots: Arc<Semaphore>) -> Self {
        self.inner = self.inner.with_limiter(slots);
        self
    }
}

#[async_trait]
impl LlmProvider for HarnessLlm {
    async fn complete(&self, request: Value, _conn: Option<&str>) -> Result<Value> {
        let instruction = instruction_of(&request)?;
        self.inner.record_prompt(&request, &instruction);
        let choice = self.inner.choice_for("agent node", &request)?;
        self.inner
            .run_on_harness(AgentRoute::Default, instruction, choice)
            .await
    }
}
