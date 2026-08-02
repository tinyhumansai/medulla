//! Turning engine run callbacks into Medulla's own work events.
//!
//! Medulla already knows how to show an agent working: a harness emits
//! [`harness_work`](crate::harness_work) semantic events, a `WorkFold` reduces
//! them into a `WorkSnapshot`, and `ui::work` renders it. A workflow run is the
//! same story told by a different narrator — a plan, steps moving through it,
//! sub-agents, a result — so this observer speaks that vocabulary rather than
//! inventing a second one. Live workflow progress therefore renders through the
//! existing pane with no new drawing code.
//!
//! The mapping:
//!
//! | engine callback | emitted kind |
//! | --- | --- |
//! | `on_run_start` | `plan_update` — one step per node, in graph order |
//! | `on_step_finish` | `todo_update` — that node settled, the next one active |
//! | `on_step_finish` (agent nodes) | `subagent_start` + a settling `tool_result` |
//! | `on_run_finish` | `run_result` |
//!
//! One wrinkle worth stating plainly: at the pinned engine version there is no
//! `on_step_start` callback, so "which node is running now" is inferred — after
//! each step settles, the first node still pending is marked active. For a
//! sequential graph that is exact; for a parallel fan-out it names one of the
//! several in flight. It is never wrong about what has *finished*, which is what
//! a progress view is mostly read for.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tinyflows::model::{NodeKind, WorkflowGraph};
use tinyflows::observability::{ExecutionStep, Run, RunObserver, RunStatus, StepStatus};

use crate::harness_work::kinds;

/// Where emitted work events go.
///
/// A callback rather than a channel so the caller decides whether an event is
/// forwarded to the orchestrator, folded into a local snapshot, or both — the
/// observer should not have to know which.
pub type WorkEventSink = Arc<dyn Fn(&str, Value) + Send + Sync>;

/// A sink that discards everything, for runs nobody is watching.
pub fn null_sink() -> WorkEventSink {
    Arc::new(|_, _| {})
}

/// One node's place in the plan.
#[derive(Debug, Clone)]
struct PlanNode {
    /// The engine's node id.
    id: String,
    /// The label shown to an operator: the node's name, or its id when unnamed.
    label: String,
    /// Whether this node dispatches to a harness, and so is also reported as a
    /// sub-agent.
    is_agent: bool,
    /// `pending`, `in_progress`, `completed`, or `cancelled`.
    status: &'static str,
}

/// A [`RunObserver`] that emits Medulla work events and records each step.
pub struct WorkflowRunObserver {
    sink: WorkEventSink,
    /// The plan, in graph order, with each node's current status.
    plan: Mutex<Vec<PlanNode>>,
    /// Steps as the engine reports them, kept so a caller can persist the run
    /// record without observing every callback itself.
    steps: Mutex<Vec<crate::workflows::RunStep>>,
    /// The same steps as the engine's own richer type.
    ///
    /// Diagnosis consumes the engine's status and diagnostic types directly.
    /// Accumulated here rather than taken from [`Run::steps`] on finish because
    /// a run that is cancelled or times out never reaches `on_run_finish`, and
    /// its partial evidence is the evidence most worth keeping.
    raw: Mutex<Vec<ExecutionStep>>,
    /// The one-line summary built when the run settled.
    ///
    /// Emitted as a `run_result` event and, until now, discarded with it — so
    /// every reader after the fact had to re-derive from `steps` a sentence the
    /// observer had already written.
    summary: Mutex<Option<String>>,
    /// The workflow this run belongs to, used only for event context.
    workflow_id: String,
}

impl WorkflowRunObserver {
    /// An observer over `graph`, emitting into `sink`.
    pub fn new(workflow_id: impl Into<String>, graph: &WorkflowGraph, sink: WorkEventSink) -> Self {
        let plan = graph
            .nodes
            .iter()
            // The trigger is not work: it is the reason the work started, and
            // listing it as a step to complete reads oddly in a progress view.
            .filter(|node| node.kind != NodeKind::Trigger)
            .map(|node| PlanNode {
                id: node.id.clone(),
                label: if node.name.trim().is_empty() {
                    node.id.clone()
                } else {
                    node.name.clone()
                },
                is_agent: node.kind == NodeKind::Agent,
                status: "pending",
            })
            .collect();

        Self {
            sink,
            plan: Mutex::new(plan),
            steps: Mutex::new(Vec::new()),
            raw: Mutex::new(Vec::new()),
            summary: Mutex::new(None),
            workflow_id: workflow_id.into(),
        }
    }

    /// The steps recorded so far, in completion order.
    pub fn steps(&self) -> Vec<crate::workflows::RunStep> {
        self.steps.lock().expect("steps lock").clone()
    }

    /// The engine's own steps, in completion order.
    ///
    /// What [`crate::workflows::run::diagnose::diagnose`] needs: it reads each
    /// step's `output` to find errors an `on_error` policy swallowed, and the
    /// graph to find nodes that never ran at all.
    pub fn execution_steps(&self) -> Vec<ExecutionStep> {
        self.raw.lock().expect("raw steps lock").clone()
    }

    /// The summary the observer wrote when the run settled.
    ///
    /// `None` when the run never settled through the engine — cancelled, timed
    /// out, or dropped with the process — in which case the caller has to say
    /// what happened itself.
    pub fn summary(&self) -> Option<String> {
        self.summary.lock().expect("summary lock").clone()
    }

    /// Emit the current plan as a `todo_update`.
    fn emit_todos(&self, plan: &[PlanNode]) {
        let todos: Vec<Value> = plan
            .iter()
            .map(|node| json!({ "content": node.label, "status": node.status }))
            .collect();
        (self.sink)(
            kinds::TODO_UPDATE,
            json!({ "todos": todos, "source": "workflow" }),
        );
    }
}

impl RunObserver for WorkflowRunObserver {
    fn on_run_start(&self, run_id: &str) {
        let plan = self.plan.lock().expect("plan lock");
        let steps: Vec<Value> = plan
            .iter()
            .map(|node| json!({ "content": node.label, "status": node.status }))
            .collect();
        (self.sink)(
            kinds::PLAN_UPDATE,
            json!({
                "goal": format!("workflow {} (run {run_id})", self.workflow_id),
                "steps": steps,
            }),
        );
    }

    fn on_step_finish(&self, step: &ExecutionStep) {
        let diagnostics: Vec<String> = step
            .diagnostics
            .iter()
            .map(|d| format!("{} resolved to null ({})", d.location, d.expression))
            .collect();

        self.steps
            .lock()
            .expect("steps lock")
            .push(crate::workflows::RunStep {
                node_id: step.node_id.clone(),
                status: match step.status {
                    StepStatus::Success => "success".to_string(),
                    StepStatus::Error => "error".to_string(),
                },
                duration_ms: step.duration_ms,
                input: None,
                output: Some(crate::workflows::bounded_evidence(&step.output)),
                diagnostics: diagnostics.clone(),
            });
        self.raw.lock().expect("raw steps lock").push(step.clone());

        // A null-resolving expression is almost always a wiring mistake the
        // author wants to hear about, but never a reason to fail a run the
        // engine was happy with.
        for diagnostic in &diagnostics {
            tracing::warn!(
                workflow = %self.workflow_id,
                node = %step.node_id,
                "workflow binding: {diagnostic}"
            );
        }

        let mut plan = self.plan.lock().expect("plan lock");
        let mut agent_finished = None;
        for node in plan.iter_mut() {
            if node.id == step.node_id {
                node.status = match step.status {
                    StepStatus::Success => "completed",
                    StepStatus::Error => "cancelled",
                };
                if node.is_agent {
                    agent_finished = Some((node.id.clone(), node.label.clone()));
                }
                break;
            }
        }
        // See the module note: with no start callback, the first still-pending
        // node is reported as the active one.
        if let Some(next) = plan.iter_mut().find(|node| node.status == "pending") {
            next.status = "in_progress";
        }
        self.emit_todos(&plan);
        drop(plan);

        if let Some((id, label)) = agent_finished {
            let call_id = format!("{}:{id}", self.workflow_id);
            (self.sink)(
                kinds::SUBAGENT_START,
                json!({
                    "call_id": call_id,
                    "agent_type": "workflow_node",
                    "description": label,
                }),
            );
            // Settle it in the same breath. The engine tells us about an agent
            // node only once it has *finished*, so a lone start would leave the
            // fold holding a sub-agent that never returns — and the terminal
            // snapshot on the reply frame would show work still in flight after
            // the run had ended. A sub-agent closes on an ordinary `tool_result`
            // carrying its call id.
            (self.sink)(
                "tool_result",
                json!({
                    "call_id": call_id,
                    "is_error": matches!(step.status, StepStatus::Error),
                    "output": match step.status {
                        StepStatus::Success => "completed",
                        StepStatus::Error => "failed",
                    },
                }),
            );
        }
    }

    fn on_run_finish(&self, run: &Run) {
        let ok = matches!(run.status, RunStatus::Completed);
        let duration_ms: u128 = run.steps.iter().map(|step| step.duration_ms).sum();
        let failed: Vec<&str> = run
            .steps
            .iter()
            .filter(|step| matches!(step.status, StepStatus::Error))
            .map(|step| step.node_id.as_str())
            .collect();

        let summary = if ok {
            format!(
                "workflow {} completed {} steps",
                self.workflow_id,
                run.steps.len()
            )
        } else {
            format!(
                "workflow {} failed at {}",
                self.workflow_id,
                failed.join(", ")
            )
        };

        // Keep it before emitting: the sink is host code that may do anything,
        // and the summary is wanted whether or not the event goes anywhere.
        *self.summary.lock().expect("summary lock") = Some(summary.clone());

        (self.sink)(
            kinds::RUN_RESULT,
            json!({
                "ok": ok,
                "duration_ms": duration_ms as u64,
                "num_turns": run.steps.len(),
                "summary": summary,
            }),
        );
    }
}

/// Collect emitted events into a [`WorkFold`](crate::harness_work::WorkFold),
/// returning the sink and the fold it writes into.
///
/// The convenience a local run wants: the caller gets a live snapshot without
/// wiring a channel, which is what the TUI needs to draw a run it started
/// itself.
pub fn folding_sink() -> (WorkEventSink, Arc<Mutex<crate::harness_work::WorkFold>>) {
    let fold = Arc::new(Mutex::new(crate::harness_work::WorkFold::new()));
    let target = fold.clone();
    let sink: WorkEventSink = Arc::new(move |kind: &str, payload: Value| {
        let at = crate::clock::now_millis();
        target.lock().expect("fold lock").apply(kind, &payload, at);
    });
    (sink, fold)
}

/// Emitted event payloads, grouped by kind.
pub type RecordedEvents = Arc<Mutex<HashMap<String, Vec<Value>>>>;

/// Collect emitted events by kind, for tests and for callers that forward them
/// verbatim.
pub fn recording_sink() -> (WorkEventSink, RecordedEvents) {
    let seen = Arc::new(Mutex::new(HashMap::<String, Vec<Value>>::new()));
    let target = seen.clone();
    let sink: WorkEventSink = Arc::new(move |kind: &str, payload: Value| {
        target
            .lock()
            .expect("recording lock")
            .entry(kind.to_string())
            .or_default()
            .push(payload);
    });
    (sink, seen)
}

#[cfg(test)]
#[path = "observability_tests.rs"]
mod tests;
