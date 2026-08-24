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
//! settled path already did. A kill that runs no destructors at all is caught
//! one level up, by the startup sweep in [`reconcile`].
//!
//! **A cancel can arrive from another process.** The registry is in-memory, so
//! a cancel aimed at a run this process is not executing is written onto the
//! record instead; [`watch_cancel_request`] is the half that notices.
//!
//! **Resume checks the host's record, not just the engine's.** The engine treats
//! a resume call as approval; that is too generous. [`resume_workflow`] requires
//! the caller to name at least one node the persisted record actually lists as
//! pending, so a stale or invented resume cannot walk a run past its gate.

pub mod dispatches;
mod preflight;
pub mod reconcile;
mod registry;
mod summary;

#[cfg(test)]
mod reconcile_tests;
#[cfg(test)]
mod tests;

// Run diagnosis moved to the engine crate with the run record it explains.
// Aliased as well as re-exported, so `diagnose::Diagnosis` still resolves here.
pub use dispatches::{in_flight, InFlightDispatch};
pub(crate) use preflight::clamp_loop_iterations;
pub use reconcile::{reconcile_once, reconcile_orphans, Reconciled};
pub use registry::{cancel, is_running, CancelSignal, RunClaim, RunGuard};
pub use summary::summarize;
use tinyflows::diagnostics as diagnose;
pub use tinyflows::diagnostics::{
    capturing, diagnose, CapturingObserver, Diagnosis, DryRun, HiddenError, NeverRan, NullBinding,
};

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value};

use crate::flow_engine::execute;
use crate::flow_engine::observability::{StepSnapshot, WorkEventSink, WorkflowRunObserver};
use crate::flow_engine::{
    agent_evidence, build_capabilities_with_agent_evidence, open_checkpointer, CapabilitySettings,
    HostServices,
};
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
    /// Persists completed-step snapshots while the run is still executing.
    pub step_snapshot: Option<StepSnapshot>,
    /// Who asked for this run, when the caller can say.
    ///
    /// Carried on the context rather than passed alongside the inputs because
    /// it is a property of the *door* the run came through, which every caller
    /// already knows once and none of them learn per run.
    pub origin: Option<crate::workflows::RunOrigin>,
    /// A registration the caller already took out for this run id.
    ///
    /// A caller that hands the run id back before the run future is first
    /// polled — [`crate::workflows::local::LocalRun::start`], which spawns and
    /// returns — has to claim the id *itself*, or a cancel arriving in that
    /// window finds an empty registry and reports having cancelled nothing
    /// while the run carries on. Such a caller claims before it returns the id
    /// and passes the claim here; the run adopts it instead of taking out a
    /// second one, which would collide with the first and refuse the run.
    ///
    /// `None` for a caller that runs the future inline, where claiming at the
    /// top of the run is already early enough.
    pub claim: Option<registry::RunClaim>,
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

/// How often a running workflow re-reads its own record for a cancel request.
///
/// A compromise between a cancel from another process feeling immediate and not
/// re-reading a file on a tight loop for the whole life of a run that may last
/// hours. A workflow step is a harness session; two seconds is noise against it.
const CANCEL_POLL: Duration = Duration::from_secs(2);

/// Resolve when someone marks this run cancelled from outside this process.
///
/// Polls rather than watches the file: the store is a trait with no change
/// notification, and a filesystem watcher would be a platform-specific
/// dependency for something a two-second poll answers well enough.
///
/// A read that fails is ignored rather than treated as a cancel. The record is
/// rewritten as the run progresses, so a transient failure to read it — a
/// half-written file, a momentary permission problem — must not be what stops
/// a healthy run.
async fn watch_cancel_request(store: Arc<dyn WorkflowStore>, run_id: String) {
    loop {
        tokio::time::sleep(CANCEL_POLL).await;
        let requested = {
            let store = store.clone();
            let run_id = run_id.clone();
            tokio::task::spawn_blocking(move || {
                matches!(store.get_run(&run_id), Ok(Some(record)) if record.cancel_requested)
            })
            .await
            .unwrap_or(false)
        };
        if requested {
            return;
        }
    }
}

/// Run the workflow `workflow_id` to completion, an approval gate, or a failure.
///
/// `run_id` doubles as the engine checkpointer's thread id and as the
/// `abort_id` on every task the run dispatches, which is what makes one abort
/// stop both the run and the harness session it is waiting on.
///
/// `input` is the free-form trigger payload; `inputs` supplies values for the
/// workflow's *declared* inputs by name. The declared values are resolved
/// before the run is claimed or recorded, so a caller that supplies a bad set
/// gets an error and leaves no run record — the same contract the engine
/// offers, enforced one layer earlier so the operator-visible history stays
/// clean.
pub async fn run_workflow(
    context: RunContext,
    workflow_id: &str,
    run_id: &str,
    input: Value,
    inputs: Map<String, Value>,
) -> Result<RunRecord, WorkflowError> {
    run_workflow_inner(context, workflow_id, run_id, input, inputs, None).await
}

/// Run a workflow only when its persisted definition still has `fingerprint`.
///
/// Remote fleet dispatches use this after selecting a worker capability advert.
/// The comparison happens against the same loaded record the engine compiles,
/// so a same-id workflow changed after discovery is refused rather than run.
pub async fn run_workflow_versioned(
    context: RunContext,
    workflow_id: &str,
    run_id: &str,
    input: Value,
    inputs: Map<String, Value>,
    fingerprint: &str,
) -> Result<RunRecord, WorkflowError> {
    run_workflow_inner(
        context,
        workflow_id,
        run_id,
        input,
        inputs,
        Some(fingerprint),
    )
    .await
}

/// Shared execution body for local and definition-bound remote runs.
async fn run_workflow_inner(
    mut context: RunContext,
    workflow_id: &str,
    run_id: &str,
    input: Value,
    inputs: Map<String, Value>,
    expected_fingerprint: Option<&str>,
) -> Result<RunRecord, WorkflowError> {
    if !context.settings.enabled {
        return Err(WorkflowError::Engine(
            "workflows are disabled on this host (workflows.enabled = false)".to_string(),
        ));
    }
    let workflow = require(context.store.as_ref(), workflow_id)?;
    if expected_fingerprint
        .is_some_and(|expected| crate::workflows::record_fingerprint(&workflow) != expected)
    {
        return Err(WorkflowError::Engine(format!(
            "workflow '{workflow_id}' changed after it was selected; refresh the worker catalog"
        )));
    }
    if !workflow.enabled {
        return Err(WorkflowError::Engine(format!(
            "workflow '{workflow_id}' is disabled"
        )));
    }
    preflight::refuse_unresolved_harness(&workflow)?;

    let settings = preflight::settings_for(&context.settings, &workflow)?;

    // Resolved here, before the run is claimed or a record written, so a bad
    // call leaves no trace in the operator's run history. The engine re-checks
    // the same values — it owns the contract — but by then a record exists.
    let resolved_inputs = tinyflows::model::resolve_inputs(&workflow.graph.inputs, &inputs)
        .map_err(|err| WorkflowError::Engine(err.to_string()))?;

    let execution_graph = preflight::clamp_loop_iterations(
        agent_evidence::instrumented(&workflow.graph),
        settings.max_loop_iterations,
    );
    let compiled = execute::compile(&execution_graph).map_err(WorkflowError::Engine)?;

    // Claimed before anything can await, so a cancel arriving during setup is
    // not dropped on the floor — and so two dispatches of the same run id
    // cannot both start, which would run every node's side effects twice and
    // race to overwrite one run record.
    let (_guard, cancelled) = preflight::adopt_or_claim(run_id, context.claim.take())?;

    // The *resolved* inputs, not the supplied ones: a caller that relied on a
    // declared default gets a record saying what the run actually used, which
    // is the value an operator would otherwise have to reconstruct from the
    // graph. Written onto the first record rather than only the settled one, so
    // a run that is still going — or that never settles — still says what it
    // was asked to do.
    let record = new_run_record(run_id, workflow_id, crate::clock::now_millis() as u64)
        .with_inputs(&resolved_inputs, &input)
        .with_origin(context.origin.clone())
        // Stamped before the first write, so there is no window in which a
        // record claims to be running without saying who is running it. A
        // record in that window would look like an orphan to a sweep in
        // another process.
        .with_executor(Some(reconcile::current_executor().clone()));
    context.store.record_run(&record)?;
    let mut finalizer = RunFinalizer::new(context.store.clone(), record.clone());

    let observer = Arc::new(
        WorkflowRunObserver::new(workflow_id, &workflow.graph, context.sink)
            .with_step_snapshot(context.step_snapshot),
    );
    let agent_evidence = Arc::new(agent_evidence::AgentEvidence::default());
    let capabilities = build_capabilities_with_agent_evidence(
        settings.clone(),
        context.services,
        &format!("workflow:{workflow_id}"),
        run_id,
        agent_evidence.clone(),
    );
    let as_observer = observer.clone() as Arc<dyn tinyflows::observability::RunObserver>;

    let engine_run = execute::run(
        &compiled,
        tinyflows::engine::RunInput::new(input).with_inputs(resolved_inputs),
        &capabilities,
        open_checkpointer(&settings),
        run_id,
        &as_observer,
    );
    let bounded = tokio::time::timeout(Duration::from_secs(settings.run_timeout_secs), engine_run);
    tokio::pin!(bounded);

    // A cancel aimed at this run from another process cannot reach the
    // in-memory registry, so it is written to the record instead. Watching for
    // it here is what makes `medulla workflow cancel` work from any shell
    // rather than only from the one that happens to be executing the run.
    let watch = watch_cancel_request(context.store.clone(), run_id.to_string());
    tokio::pin!(watch);

    // `biased` so a cancel that lands in the same poll as the run settling wins
    // deterministically, rather than depending on which future the runtime
    // happened to look at first.
    let settled = tokio::select! {
        biased;
        _ = cancelled.cancelled() => Err(Settle::Cancelled),
        _ = &mut watch => Err(Settle::Cancelled),
        result = &mut bounded => match result {
            Ok(Ok(outcome)) => Ok(outcome),
            Ok(Err(err)) => Err(Settle::Failed(err)),
            Err(_) => Err(Settle::TimedOut(settings.run_timeout_secs)),
        },
    };

    let mut record = record;
    record.steps = observer.steps();
    agent_evidence.attach(&mut record.steps);
    record.finished_at = Some(crate::clock::now_millis() as u64);
    let terminal_engine_error = matches!(&settled, Err(Settle::Failed(_)));
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
    record_evidence(
        &mut record,
        &observer,
        &workflow.graph,
        terminal_engine_error,
    );

    // Written *before* disarming: if this terminal write fails, the guard must
    // still be armed so its drop reconciles the record to `Interrupted`.
    // Disarming first would strand the run at `Running` forever.
    context.store.record_run(&record)?;
    finalizer.disarm();
    remember_failure(&context.store, &record);
    Ok(record)
}

/// Write the observation a failed run earns, before anyone asks for one.
///
/// The sync half of the evolution trigger, and deliberately the only half that
/// lives here. This body is shared by the CLI, the daemon, and the TUI; it
/// holds no dispatch and no worker address, so starting a harness from it would
/// make `medulla workflow run` silently spawn a second agent. What it *can* do
/// everywhere is turn a run record into a note.
///
/// Only for a run that settles as `Failed`. A run reconciled to `Interrupted`
/// by `RunFinalizer::drop` gets none: the process was going away, there is no
/// diagnosis to write down, and "we were killed" teaches a workflow nothing
/// about itself.
///
/// Best effort by design: a note that could not be written must not turn a run
/// that already happened into a failure to record it.
fn remember_failure(store: &Arc<dyn WorkflowStore>, record: &RunRecord) {
    if record.status != RunStatus::Failed {
        return;
    }
    if let Err(err) = crate::workflows::evolve::record_failure_note(store, record) {
        tracing::warn!(
            run = %record.id,
            workflow = %record.workflow_id,
            "could not record what this failure taught: {err}"
        );
    }
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
    preflight::refuse_unresolved_harness(&workflow)?;
    let settings = preflight::settings_for(&context.settings, &workflow)?;
    let execution_graph = preflight::clamp_loop_iterations(
        agent_evidence::instrumented(&workflow.graph),
        settings.max_loop_iterations,
    );
    let compiled = execute::compile(&execution_graph).map_err(WorkflowError::Engine)?;

    let Some((_guard, cancelled)) = RunGuard::claim(run_id) else {
        return Err(WorkflowError::Engine(format!(
            "run '{run_id}' is already executing"
        )));
    };
    record.status = RunStatus::Running;
    record.finished_at = None;
    context.store.record_run(&record)?;
    let mut finalizer = RunFinalizer::new(context.store.clone(), record.clone());

    let observer = Arc::new(
        WorkflowRunObserver::new(&record.workflow_id, &workflow.graph, context.sink)
            .with_step_snapshot(context.step_snapshot),
    );
    let agent_evidence = Arc::new(agent_evidence::AgentEvidence::default());
    let capabilities = build_capabilities_with_agent_evidence(
        settings.clone(),
        context.services,
        &format!("workflow:{}", record.workflow_id),
        run_id,
        agent_evidence.clone(),
    );
    let as_observer = observer.clone() as Arc<dyn tinyflows::observability::RunObserver>;

    let engine_run = execute::resume(
        &compiled,
        &capabilities,
        open_checkpointer(&settings),
        run_id,
        approvals,
        rejections,
        &as_observer,
    );
    let bounded = tokio::time::timeout(Duration::from_secs(settings.run_timeout_secs), engine_run);
    tokio::pin!(bounded);

    let settled = tokio::select! {
        biased;
        _ = cancelled.cancelled() => Err(Settle::Cancelled),
        result = &mut bounded => match result {
            Ok(Ok(outcome)) => Ok(outcome),
            Ok(Err(err)) => Err(Settle::Failed(err)),
            Err(_) => Err(Settle::TimedOut(settings.run_timeout_secs)),
        },
    };

    // Steps from this leg are appended: the record is the whole run's history,
    // not just its latest attempt.
    let earlier_steps = record.steps.len();
    let mut resumed_steps = observer.steps();
    agent_evidence.attach(&mut resumed_steps);
    record.steps.extend(resumed_steps);
    record.finished_at = Some(crate::clock::now_millis() as u64);
    let terminal_engine_error = matches!(&settled, Err(Settle::Failed(_)));
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
    let resumed_steps = observer.execution_steps();
    let resumed = diagnose_record(&workflow.graph, &resumed_steps, terminal_engine_error);
    record.diagnosis = Some(match record.diagnosis.take() {
        Some(earlier) => merge_diagnoses(earlier, resumed, &record.steps[..earlier_steps]),
        None => resumed,
    });
    // The observer only saw this resumed leg; the record holds the complete
    // pre-gate and post-gate history.
    record.summary = Some(summary::summarize(&record));

    // Written before disarming, for the same reason as `run_workflow`: a
    // terminal write that fails must leave the drop guard armed to reconcile
    // the record rather than leaving a resumed run stuck at `Running`.
    context.store.record_run(&record)?;
    finalizer.disarm();
    remember_failure(&context.store, &record);
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
    terminal_engine_error: bool,
) {
    // The observer's own sentence when it has one: it saw the engine settle and
    // can count what actually ran. The fallback only knows the record.
    record.summary = final_summary(record, observer);
    let steps = observer.execution_steps();
    record.diagnosis = Some(diagnose_record(graph, &steps, terminal_engine_error));
}

/// Diagnose a settled run without calling its terminal error "swallowed".
fn diagnose_record(
    graph: &tinyflows::model::WorkflowGraph,
    steps: &[tinyflows::observability::ExecutionStep],
    failed: bool,
) -> diagnose::Diagnosis {
    let mut diagnosis = diagnose::diagnose(graph, steps);
    if failed {
        let terminal = steps
            .iter()
            .rev()
            .find(|step| matches!(step.status, tinyflows::observability::StepStatus::Error));
        if let Some(position) = terminal.and_then(|step| {
            diagnosis
                .hidden_errors
                .iter()
                .rposition(|error| error.node_id == step.node_id)
        }) {
            diagnosis.hidden_errors.remove(position);
        }
    }
    diagnosis
}

/// Prefer the observer's richer summary except when a run paused for approval.
///
/// The observer only knows whether engine execution returned successfully; it
/// cannot describe the host-level `PendingApproval` state or name its gates.
fn final_summary(record: &RunRecord, observer: &WorkflowRunObserver) -> Option<String> {
    if record.status == RunStatus::PendingApproval {
        return Some(summary::summarize(record));
    }
    observer
        .summary()
        .or_else(|| Some(summary::summarize(record)))
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
    inputs: Map<String, Value>,
) -> Result<DryRun, WorkflowError> {
    let workflow = require(store.as_ref(), workflow_id)?;
    dry_run_graph(&workflow.graph, resolver, input, inputs).await
}

/// Simulate a graph that is not necessarily saved anywhere.
///
/// The same check as [`dry_run`], against a candidate rather than a record.
/// What a proposal needs: the whole question is whether a patched graph *would*
/// work, and answering it by saving the patch first would be the silent
/// mutation the proposal exists to avoid.
///
/// # Errors
///
/// Returns [`WorkflowError`] if the graph cannot be compiled or simulated.
pub async fn dry_run_graph(
    graph: &tinyflows::model::WorkflowGraph,
    resolver: Arc<dyn tinyflows::caps::WorkflowResolver>,
    input: Value,
    inputs: Map<String, Value>,
) -> Result<DryRun, WorkflowError> {
    let compiled = execute::compile(graph).map_err(WorkflowError::Engine)?;
    let capabilities = crate::flow_engine::build_dry_run_capabilities(resolver);

    // Observed, because the steps are what a dry run is *for* at authoring
    // time. The outcome only says the graph completed; a binding that resolved
    // to null on the way completes too.
    let (capture, observer) = diagnose::capturing();
    let outcome = execute::simulate_observed(
        &compiled,
        tinyflows::engine::RunInput::new(input).with_inputs(inputs),
        &capabilities,
        &observer,
    )
    .await
    .map_err(WorkflowError::Engine)?;

    Ok(DryRun {
        output: outcome.output,
        diagnosis: diagnose::diagnose(graph, &capture.steps()),
    })
}
