//! Running a workflow, and resuming one that paused.
//!
//! One body ([`run_workflow`]) drives every run, so the bookkeeping — the run
//! record, the cancellation registration, the reconciliation of a run whose
//! process went away — happens once rather than once per caller.
//!
//! Three things here are worth knowing before changing them:
//!
//! **Cancellation is host-level, not engine-level.** At the pinned engine
//! version no entry point takes both a checkpointer and a cancellation token,
//! and durable approval pauses need the checkpointer. So a cancel drops the run
//! future rather than asking the engine to wind down. That is less tidy but
//! costs nothing here, because a workflow's real work is a harness session, and
//! those are aborted directly: every dispatched task carries the run id as its
//! `abort_id`, so the orchestrator's ordinary abort path stops the node in
//! flight whether or not this process is still waiting on it.
//!
//! **The run record is reconciled on drop.** A run that is cancelled, panics, or
//! dies with the process would otherwise leave a record claiming to be
//! `running` forever. `RunFinalizer` writes a terminal status on drop unless a
//! settled path already did.
//!
//! **Resume checks the host's record, not just the engine's.** The engine treats
//! a resume call as approval; that is too generous. [`resume_workflow`] requires
//! the caller to name at least one node the persisted record actually lists as
//! pending, so a stale or invented resume cannot walk a run past its gate.

pub mod diagnose;
mod registry;
mod summary;

#[cfg(test)]
mod tests;

pub use diagnose::{diagnose, Diagnosis, DryRun, HiddenError, NeverRan, NullBinding};
pub use registry::{cancel, is_running, RunGuard};
pub use summary::summarize;

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::flow_engine::execute;
use crate::flow_engine::observability::{WorkEventSink, WorkflowRunObserver};
use crate::flow_engine::{build_capabilities, open_checkpointer, CapabilitySettings, HostServices};
use crate::workflows::store::{new_run_record, require, require_run};
use crate::workflows::{RunRecord, RunStatus, RunStep, WorkflowError, WorkflowStore};

/// Everything one run needs beyond the workflow itself.
pub struct RunContext {
    /// The store the definition came from and the run record goes to.
    pub store: Arc<dyn WorkflowStore>,
    /// What the run's capabilities may do.
    pub settings: Arc<CapabilitySettings>,
    /// Where `agent` nodes dispatch and how sub-workflows resolve.
    pub services: HostServices,
    /// Where progress events go.
    pub sink: WorkEventSink,
}

/// Write a terminal status onto a run record unless a settled path already did.
///
/// A drop guard rather than a `finally`: the run future can be dropped at any
/// await point — a cancel, a shutdown, a panic — and every one of those must
/// still leave the record honest.
struct RunFinalizer {
    store: Arc<dyn WorkflowStore>,
    record: RunRecord,
    armed: bool,
}

impl RunFinalizer {
    fn new(store: Arc<dyn WorkflowStore>, record: RunRecord) -> Self {
        Self {
            store,
            record,
            armed: true,
        }
    }

    /// Stop the guard from writing: a settled path is about to write its own,
    /// more accurate, terminal status.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RunFinalizer {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.record.status = RunStatus::Interrupted;
        self.record.finished_at = Some(crate::clock::now_millis() as u64);
        // Best effort: a run already ending badly should not also panic in a
        // destructor because the disk is full.
        if let Err(err) = self.store.record_run(&self.record) {
            tracing::warn!(run = %self.record.id, "could not reconcile run record: {err}");
        }
    }
}

/// Run the workflow `workflow_id` to completion, an approval gate, or a failure.
///
/// `run_id` doubles as the engine checkpointer's thread id and as the
/// `abort_id` on every task the run dispatches, which is what makes one abort
/// stop both the run and the harness session it is waiting on.
pub async fn run_workflow(
    context: RunContext,
    workflow_id: &str,
    run_id: &str,
    input: Value,
) -> Result<RunRecord, WorkflowError> {
    if !context.settings.enabled {
        return Err(WorkflowError::Engine(
            "workflows are disabled on this host (workflows.enabled = false)".to_string(),
        ));
    }
    let workflow = require(context.store.as_ref(), workflow_id)?;
    if !workflow.enabled {
        return Err(WorkflowError::Engine(format!(
            "workflow '{workflow_id}' is disabled"
        )));
    }

    let compiled = execute::compile(&workflow.graph).map_err(WorkflowError::Engine)?;

    // Claimed before anything can await, so a cancel arriving during setup is
    // not dropped on the floor — and so two dispatches of the same run id
    // cannot both start, which would run every node's side effects twice and
    // race to overwrite one run record.
    let Some((_guard, cancelled)) = RunGuard::claim(run_id) else {
        return Err(WorkflowError::Engine(format!(
            "run '{run_id}' is already executing"
        )));
    };

    let record = new_run_record(run_id, workflow_id, crate::clock::now_millis() as u64);
    context.store.record_run(&record)?;
    let mut finalizer = RunFinalizer::new(context.store.clone(), record.clone());

    let observer = Arc::new(WorkflowRunObserver::new(
        workflow_id,
        &workflow.graph,
        context.sink,
    ));
    let capabilities = build_capabilities(
        context.settings.clone(),
        context.services,
        &format!("workflow:{workflow_id}"),
        run_id,
    );
    let as_observer = observer.clone() as Arc<dyn tinyflows::observability::RunObserver>;

    let engine_run = execute::run(
        &compiled,
        input,
        &capabilities,
        open_checkpointer(&context.settings),
        run_id,
        &as_observer,
    );
    let bounded = tokio::time::timeout(
        Duration::from_secs(context.settings.run_timeout_secs),
        engine_run,
    );
    tokio::pin!(bounded);

    // `biased` so a cancel that lands in the same poll as the run settling wins
    // deterministically, rather than depending on which future the runtime
    // happened to look at first.
    let settled = tokio::select! {
        biased;
        _ = cancelled.cancelled() => Err(Settle::Cancelled),
        result = &mut bounded => match result {
            Ok(Ok(outcome)) => Ok(outcome),
            Ok(Err(err)) => Err(Settle::Failed(err)),
            Err(_) => Err(Settle::TimedOut(context.settings.run_timeout_secs)),
        },
    };

    let mut record = record;
    record.steps = observer.steps();
    record.finished_at = Some(crate::clock::now_millis() as u64);
    match settled {
        Ok(outcome) => {
            record.pending_approvals = outcome.pending_approvals.clone();
            record.status = if outcome.cancelled {
                RunStatus::Cancelled
            } else if outcome.pending_approvals.is_empty() {
                RunStatus::Succeeded
            } else {
                RunStatus::PendingApproval
            };
        }
        Err(Settle::Cancelled) => record.status = RunStatus::Cancelled,
        Err(Settle::TimedOut(secs)) => {
            record.status = RunStatus::Failed;
            record.error = Some(format!("run exceeded its {secs}s limit"));
        }
        Err(Settle::Failed(message)) => {
            record.status = RunStatus::Failed;
            record.error = Some(message);
        }
    }
    record_evidence(&mut record, &observer, &workflow.graph);

    finalizer.disarm();
    context.store.record_run(&record)?;
    Ok(record)
}

/// How a run stopped, when it did not produce an outcome.
enum Settle {
    /// An operator or an abort frame cancelled it.
    Cancelled,
    /// It ran past the host's wall-clock limit.
    TimedOut(u64),
    /// The engine returned an error.
    Failed(String),
}

/// Resume a run that paused on one or more approval gates.
///
/// `approvals` names the gates being approved. At least one must be a gate the
/// persisted record actually lists as pending: the engine treats the resume call
/// itself as consent, so without this check any resume would release every gate,
/// and a stale request could walk a run past an approval nobody gave.
pub async fn resume_workflow(
    context: RunContext,
    run_id: &str,
    approvals: Vec<String>,
    rejections: Vec<String>,
) -> Result<RunRecord, WorkflowError> {
    if !context.settings.enabled {
        return Err(WorkflowError::Engine(
            "workflows are disabled on this host (workflows.enabled = false)".to_string(),
        ));
    }
    let mut record = require_run(context.store.as_ref(), run_id)?;
    if record.status != RunStatus::PendingApproval {
        return Err(WorkflowError::Engine(format!(
            "run '{run_id}' is {:?}, not awaiting approval",
            record.status
        )));
    }
    // Either decision counts: rejecting the only gate a run is holding is a
    // legitimate way to settle it, so requiring an approval would make
    // `--reject` unusable on a single-gate workflow.
    let names_a_pending_gate = approvals
        .iter()
        .chain(rejections.iter())
        .any(|id| record.pending_approvals.contains(id));
    if !names_a_pending_gate {
        return Err(WorkflowError::Engine(format!(
            "run '{run_id}' is waiting on {}; the decisions given name none of them",
            record.pending_approvals.join(", ")
        )));
    }

    let workflow = require(context.store.as_ref(), &record.workflow_id)?;
    let compiled = execute::compile(&workflow.graph).map_err(WorkflowError::Engine)?;

    let Some((_guard, cancelled)) = RunGuard::claim(run_id) else {
        return Err(WorkflowError::Engine(format!(
            "run '{run_id}' is already executing"
        )));
    };
    record.status = RunStatus::Running;
    record.finished_at = None;
    context.store.record_run(&record)?;
    let mut finalizer = RunFinalizer::new(context.store.clone(), record.clone());

    let observer = Arc::new(WorkflowRunObserver::new(
        &record.workflow_id,
        &workflow.graph,
        context.sink,
    ));
    let capabilities = build_capabilities(
        context.settings.clone(),
        context.services,
        &format!("workflow:{}", record.workflow_id),
        run_id,
    );
    let as_observer = observer.clone() as Arc<dyn tinyflows::observability::RunObserver>;

    let engine_run = execute::resume(
        &compiled,
        &capabilities,
        open_checkpointer(&context.settings),
        run_id,
        approvals,
        rejections,
        &as_observer,
    );
    let bounded = tokio::time::timeout(
        Duration::from_secs(context.settings.run_timeout_secs),
        engine_run,
    );
    tokio::pin!(bounded);

    let settled = tokio::select! {
        biased;
        _ = cancelled.cancelled() => Err(Settle::Cancelled),
        result = &mut bounded => match result {
            Ok(Ok(outcome)) => Ok(outcome),
            Ok(Err(err)) => Err(Settle::Failed(err)),
            Err(_) => Err(Settle::TimedOut(context.settings.run_timeout_secs)),
        },
    };

    // Steps from this leg are appended: the record is the whole run's history,
    // not just its latest attempt.
    let earlier_steps = record.steps.len();
    record.steps.extend(observer.steps());
    record.finished_at = Some(crate::clock::now_millis() as u64);
    match settled {
        Ok(outcome) => {
            record.pending_approvals = outcome.pending_approvals.clone();
            record.status = if outcome.cancelled {
                RunStatus::Cancelled
            } else if outcome.pending_approvals.is_empty() {
                RunStatus::Succeeded
            } else {
                RunStatus::PendingApproval
            };
        }
        Err(Settle::Cancelled) => record.status = RunStatus::Cancelled,
        Err(Settle::TimedOut(secs)) => {
            record.status = RunStatus::Failed;
            record.error = Some(format!("run exceeded its {secs}s limit"));
        }
        Err(Settle::Failed(message)) => {
            record.status = RunStatus::Failed;
            record.error = Some(message);
        }
    }
    // A resumed leg only observed the nodes that ran *after* the gate, so its
    // diagnosis would report every earlier node as one that never ran. Merged
    // rather than replaced, for the same reason the steps are appended.
    let resumed = diagnose::diagnose(&workflow.graph, &observer.execution_steps());
    record.diagnosis = Some(match record.diagnosis.take() {
        Some(earlier) => merge_diagnoses(earlier, resumed, &record.steps[..earlier_steps]),
        None => resumed,
    });
    record.summary = observer
        .summary()
        .or_else(|| Some(summary::summarize(&record)));

    finalizer.disarm();
    context.store.record_run(&record)?;
    Ok(record)
}

/// Attach what the run *taught*, as distinct from whether it succeeded.
///
/// Called once the status is final, because the fallback summary reads the
/// status to phrase itself. Both fields are additive and neither can fail, so
/// this never has a say in whether a run is recorded.
fn record_evidence(
    record: &mut RunRecord,
    observer: &WorkflowRunObserver,
    graph: &tinyflows::model::WorkflowGraph,
) {
    // The observer's own sentence when it has one: it saw the engine settle and
    // can count what actually ran. The fallback only knows the record.
    record.summary = observer
        .summary()
        .or_else(|| Some(summary::summarize(record)));
    record.diagnosis = Some(diagnose::diagnose(graph, &observer.execution_steps()));
}

/// Fold a resumed leg's diagnosis into the one from before the gate.
///
/// Findings accumulate, but `never_ran` is intersected with what the resumed
/// leg still believes never ran, minus the nodes the earlier leg did run — a
/// node that executed before the pause did execute, however blind this leg was
/// to it.
fn merge_diagnoses(
    mut earlier: diagnose::Diagnosis,
    resumed: diagnose::Diagnosis,
    earlier_steps: &[RunStep],
) -> diagnose::Diagnosis {
    earlier.null_bindings.extend(resumed.null_bindings);
    earlier.empty_prompts.extend(resumed.empty_prompts);
    earlier.hidden_errors.extend(resumed.hidden_errors);
    earlier.never_ran = resumed
        .never_ran
        .into_iter()
        .filter(|missing| {
            !earlier_steps
                .iter()
                .any(|step| step.node_id == missing.node_id)
        })
        .collect();
    earlier
}

/// Validate a workflow by simulating it: every expression resolved, every
/// declared output shape satisfied, and nothing outside the process touched.
///
/// The check an author wants before saving. Static validation catches a broken
/// graph; this catches a graph that is well-formed but wired wrong.
pub async fn dry_run(
    store: Arc<dyn WorkflowStore>,
    resolver: Arc<dyn tinyflows::caps::WorkflowResolver>,
    workflow_id: &str,
    input: Value,
) -> Result<DryRun, WorkflowError> {
    let workflow = require(store.as_ref(), workflow_id)?;
    dry_run_graph(&workflow.graph, resolver, input).await
}

/// Simulate a graph that is not necessarily saved anywhere.
///
/// The same check as [`dry_run`], against a candidate rather than a record.
/// What a proposal needs: the whole question is whether a patched graph *would*
/// work, and answering it by saving the patch first would be the silent
/// mutation the proposal exists to avoid.
pub async fn dry_run_graph(
    graph: &tinyflows::model::WorkflowGraph,
    resolver: Arc<dyn tinyflows::caps::WorkflowResolver>,
    input: Value,
) -> Result<DryRun, WorkflowError> {
    let compiled = execute::compile(graph).map_err(WorkflowError::Engine)?;
    let capabilities = crate::flow_engine::build_dry_run_capabilities(resolver);

    // Observed, because the steps are what a dry run is *for* at authoring
    // time. The outcome only says the graph completed; a binding that resolved
    // to null on the way completes too.
    let (capture, observer) = diagnose::capturing();
    let outcome = execute::simulate_observed(&compiled, input, &capabilities, &observer)
        .await
        .map_err(WorkflowError::Engine)?;

    Ok(DryRun {
        output: outcome.output,
        diagnosis: diagnose::diagnose(graph, &capture.steps()),
    })
}
