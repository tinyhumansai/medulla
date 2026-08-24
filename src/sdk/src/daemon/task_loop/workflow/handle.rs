//! The frame-level lifecycle of one workflow run.
//!
//! A `task` frame naming a `workflow` reaches [`DaemonRuntime`] here: this
//! module runs the admission and validation checks an ordinary task gets
//! (enabled, at capacity, installed, not already running), acks the frame, runs
//! the saved graph through [`super::dispatch::RuntimeDispatch`] for its `agent`
//! nodes, replies with the run's outcome, and — for a run that failed — spawns
//! the detached evolution review that reads the same store the run wrote to.

use std::sync::Arc;

use crate::flow_engine::{folding_sink, CapabilitySettings, HostServices};
use crate::protocol::{TaskFrame, TaskFrameKind, WorkflowAdvert, WorkflowInputAdvert};
// `trigger_input` is shared with the cloud plane's adapter
// ([`crate::workflows::bridge`]) rather than defined twice: a frame's text must
// become the same trigger payload whether it arrived over the host link or over the
// backend socket, and two copies of that rule would eventually disagree.
use crate::workflows::bridge::trigger_input;
use crate::workflows::evolve::{EvolveConfig, EvolveSession, EvolveTrigger};
use crate::workflows::{
    run_workflow_versioned, RunContext, RunStatus, StoreWorkflowResolver, WorkflowStore,
};

use super::super::super::types::{DaemonRuntime, FrameAttachments, CAPACITY_REJECTION_PREFIX};
use super::RuntimeDispatch;

impl DaemonRuntime {
    /// The workflows this worker has installed, for its capability advert.
    ///
    /// Best effort: a store that cannot be read advertises nothing rather than
    /// failing the probe, because a worker with an unreadable workflow directory
    /// is still a perfectly good worker for ordinary tasks.
    pub(in crate::daemon) fn installed_workflows(&self) -> Vec<WorkflowAdvert> {
        // A host with workflows off advertises none: an orchestrator should not
        // be told about work it will be refused.
        if !self.workflow_settings().enabled {
            return Vec::new();
        }
        let store = self.workflow_store();
        store
            .list()
            .unwrap_or_default()
            .into_iter()
            .filter(|summary| summary.enabled)
            .filter_map(|summary| {
                let record = store.get(&summary.id).ok().flatten()?;
                Some(WorkflowAdvert {
                    id: summary.id,
                    name: summary.name,
                    description: summary.description,
                    node_count: summary.node_count,
                    fingerprint: crate::workflows::record_fingerprint(&record),
                    inputs: summary
                        .inputs
                        .into_iter()
                        .map(|input| WorkflowInputAdvert {
                            name: input.name,
                            ty: input.ty.as_str().to_string(),
                            description: input.description.unwrap_or_default(),
                            required: input.required,
                            default: input.default,
                        })
                        .collect(),
                })
            })
            .collect()
    }

    /// The workflow store for this worker: its configured workspace, layered
    /// over the user-global directory the way every other workflow surface
    /// resolves it.
    fn workflow_store(&self) -> Arc<dyn WorkflowStore> {
        let cwd = std::path::Path::new(&self.inner.config.workspace);
        Arc::new(crate::workflows::store::discover(
            &self.inner.config.env,
            cwd,
        ))
    }

    /// Run the workflow a `task` frame named, replying with its outcome.
    pub(in crate::daemon) async fn handle_workflow_task(
        &self,
        from: String,
        frame: TaskFrame,
        id: String,
    ) {
        let correlation = frame.correlation_id.clone();

        let mut settings = CapabilitySettings::clone(&self.workflow_settings());
        settings.fleet_depth = frame.fleet_depth;
        let settings = Arc::new(settings);
        if !settings.enabled {
            self.reply(
                &from,
                TaskFrameKind::Error,
                &frame.task_id,
                "workflows are disabled on this worker (workflows.enabled = false)",
                correlation.as_deref(),
                None,
            )
            .await;
            return;
        }

        // Held for the rest of the call, including the validation rejections
        // below: releasing it is the guard's job, never a hand-written
        // decrement that an unwind can skip.
        let Some(admission) = self.admit() else {
            self.reply(
                &from,
                TaskFrameKind::Error,
                &frame.task_id,
                &format!(
                    "{CAPACITY_REJECTION_PREFIX} ({} pending tasks); retry later",
                    self.inner.config.max_pending
                ),
                correlation.as_deref(),
                None,
            )
            .await;
            return;
        };

        let store = self.workflow_store();
        if store.get(&id).ok().flatten().is_none() {
            // Naming what *is* installed turns a failed dispatch into something
            // the orchestrator can correct on its next attempt.
            let known: Vec<String> = store
                .list()
                .unwrap_or_default()
                .into_iter()
                .map(|s| s.id)
                .collect();
            let known = if known.is_empty() {
                "(none installed)".to_string()
            } else {
                known.join(", ")
            };
            self.reply(
                &from,
                TaskFrameKind::Error,
                &frame.task_id,
                &format!("no workflow '{id}' on this worker; installed: {known}"),
                correlation.as_deref(),
                None,
            )
            .await;
            return;
        }

        // A resent frame (a lost ack) or a sender reusing an active id must not
        // start the graph twice: both copies would run every node's side
        // effects and race to overwrite the same run record.
        if crate::workflows::run::is_running(&frame.task_id) {
            self.reply(
                &from,
                TaskFrameKind::Error,
                &frame.task_id,
                &format!("workflow {} is already running", frame.task_id),
                correlation.as_deref(),
                None,
            )
            .await;
            return;
        }

        self.reply(
            &from,
            TaskFrameKind::Ack,
            &frame.task_id,
            "workflow accepted",
            correlation.as_deref(),
            None,
        )
        .await;
        self.log(&format!("workflow {} → {id}", frame.task_id));

        let (sink, fold) = folding_sink();
        // Kept past the move into the resolver: a failed run gets a review, and
        // the review reads the same store the run wrote to.
        let evolve_store = store.clone();
        let max_loop_iterations = settings.max_loop_iterations;
        let context = RunContext {
            // Runs inline, so claiming at the top of the run is early enough.
            claim: None,
            store: store.clone(),
            settings,
            services: HostServices {
                dispatch: Arc::new(RuntimeDispatch::new(self.clone(), from.clone())),
                node_progress: None,
                resolver: Arc::new(StoreWorkflowResolver::new(store, max_loop_iterations)),
                http_credentials: Default::default(),
            },
            sink,
            step_snapshot: None,
            // A fleet dispatch: this run exists because a peer sent a task
            // frame, and the address it came from is the only thing that can
            // say which one.
            origin: Some(
                crate::workflows::RunOrigin::of_kind("dispatch")
                    .labelled(format!("task {} from {from}", frame.task_id)),
            ),
        };

        // The frame's task id becomes the run id, so the orchestrator's existing
        // `abort` for that task is exactly what cancels the run.
        let Some(fingerprint) = frame.workflow_fingerprint.clone() else {
            self.reply(
                &from,
                TaskFrameKind::Error,
                &frame.task_id,
                "workflow dispatch is missing its definition fingerprint; refresh the worker catalog",
                correlation.as_deref(),
                None,
            )
            .await;
            return;
        };
        let outcome = run_workflow_versioned(
            context,
            &id,
            &frame.task_id,
            trigger_input(&frame.text),
            frame.workflow_inputs,
            &fingerprint,
        )
        .await;

        let work = fold.lock().ok().map(|fold| fold.snapshot().clone());
        // No session id: a workflow run is a graph, not one harness session —
        // each `agent` node opens its own. There is no single session that
        // served this task, so none is claimed.
        let attachments = FrameAttachments {
            usage: None,
            work,
            ..Default::default()
        };

        match outcome {
            Ok(record) => {
                let failed = record.status == RunStatus::Failed;
                let text = summarize(&record);
                let kind = match record.status {
                    RunStatus::Succeeded | RunStatus::PendingApproval => TaskFrameKind::Reply,
                    _ => TaskFrameKind::Error,
                };
                self.reply_with(
                    &from,
                    kind,
                    &frame.task_id,
                    &text,
                    correlation.as_deref(),
                    None,
                    attachments,
                )
                .await;
                if failed {
                    self.spawn_review(evolve_store, &id, &frame.task_id, &from);
                }
            }
            Err(err) => {
                self.reply_with(
                    &from,
                    TaskFrameKind::Error,
                    &frame.task_id,
                    &format!("workflow '{id}' failed: {err}"),
                    correlation.as_deref(),
                    None,
                    attachments,
                )
                .await;
            }
        }

        // After the terminal frame, never before: the slot this run occupied is
        // only genuinely free once the requester has been told how it ended.
        drop(admission);
    }

    /// Start a review of a workflow whose run just failed.
    ///
    /// Spawned rather than awaited, and only after the reply has gone out: a
    /// review is a whole harness turn, and the orchestrator waiting on this
    /// task should not be held for one. Nothing depends on its result here —
    /// what it produces is a note and possibly a proposal, both of which live
    /// in the store for an operator to find.
    ///
    /// The run's own failure note is *not* written here. `run_workflow` already
    /// wrote it, synchronously, so it exists whether or not this review ever
    /// starts.
    fn spawn_review(
        &self,
        store: Arc<dyn WorkflowStore>,
        workflow_id: &str,
        run_id: &str,
        from: &str,
    ) {
        let settings = self.evolve_settings();
        if !settings.enabled || !settings.auto_on_failure {
            return;
        }
        let session = EvolveSession {
            store,
            // An evolution pass is restricted to proposal tools. It is not an
            // authored workflow node and must not inherit workflow authority.
            dispatch: Arc::new(
                RuntimeDispatch::new(self.clone(), from.to_string())
                    .with_origin(crate::daemon::providers::RunTaskOrigin::DelegatedTask),
            ),
            worker_address: self.inner.config.default_provider.as_str().to_string(),
            provider: Some(self.inner.config.default_provider),
            model: self.inner.config.model.clone(),
            // Per workflow, not per run, so the attribution of successive
            // reviews reads as one thread. It does not buy harness continuity
            // here: `RuntimeDispatch` runs every task `Bounded` and ignores
            // this field. What actually carries knowledge between reviews is
            // the journal, which is the point — a note survives a restart and a
            // resumed session does not.
            conversation: format!("evolve:{workflow_id}"),
            config: settings,
        };
        let workflow_id = workflow_id.to_string();
        let trigger = EvolveTrigger::Failure(run_id.to_string());
        // `--once` waits on the same counter as inbound tasks. Register the
        // detached review before spawning it so the daemon cannot observe an
        // idle gap and exit between the workflow reply and this turn.
        self.inner
            .inflight_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let runtime = self.clone();
        tokio::spawn(async move {
            match session.evolve(&workflow_id, trigger, None).await {
                Ok(outcome) if outcome.skipped => {
                    tracing::debug!(workflow = %workflow_id, "a review is already in flight");
                }
                Ok(outcome) => tracing::info!(
                    workflow = %workflow_id,
                    notes = outcome.notes.len(),
                    proposals = outcome.proposals.len(),
                    "reviewed a failed run",
                ),
                Err(err) => {
                    tracing::warn!(workflow = %workflow_id, "review failed: {err}")
                }
            }
            if runtime
                .inner
                .inflight_count
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst)
                == 1
            {
                runtime.inner.inflight_idle.notify_waiters();
            }
        });
    }

    /// This worker's review settings, failing closed when config is unreadable.
    fn evolve_settings(&self) -> EvolveConfig {
        let cwd = std::path::Path::new(&self.inner.config.workspace);
        crate::config::load_config(
            crate::config::explicit_config_from_env(&self.inner.config.env),
            &self.inner.config.env,
            cwd,
        )
        .map(|loaded| EvolveConfig::from_config(&loaded.config.workflows))
        .unwrap_or_else(|err| {
            tracing::warn!("could not reload evolution policy; reviews disabled: {err}");
            EvolveConfig {
                enabled: false,
                auto_on_failure: false,
                ..EvolveConfig::default()
            }
        })
    }

    /// Capability settings for workflows run on this worker.
    ///
    /// Read from the operator's layered config so `workflows.enabled` and the
    /// allowlists mean the same thing here as they do on the CLI. A config that
    /// cannot be loaded fails closed, including code execution.
    fn workflow_settings(&self) -> Arc<CapabilitySettings> {
        let home = crate::home::medulla_home(&self.inner.config.env);
        let cwd = std::path::Path::new(&self.inner.config.workspace);
        let mut settings = crate::config::load_config(
            crate::config::explicit_config_from_env(&self.inner.config.env),
            &self.inner.config.env,
            cwd,
        )
        .map(|loaded| CapabilitySettings::from_config(&loaded.config.workflows, &home))
        .unwrap_or_else(|err| {
            tracing::warn!("could not reload workflow policy; code execution disabled: {err}");
            CapabilitySettings::fail_closed_at(home)
        });
        // The daemon's own workspace, which is the directory it serves tasks
        // for — the same one an `agent` node's harness session runs in.
        settings.workspace = self.inner.config.workspace.clone();
        settings.default_worker_address = self.inner.config.default_provider.as_str().to_string();
        settings.default_provider = Some(self.inner.config.default_provider);
        settings.default_model = self.inner.config.model.clone();
        Arc::new(settings)
    }
}

/// A one-line account of how a run ended, for the reply frame's text.
///
/// Delegates rather than phrasing its own: a run that is described one way in
/// the reply frame and another way in its own record is a run an operator has
/// to reconcile by hand.
fn summarize(record: &crate::workflows::RunRecord) -> String {
    if record.status == RunStatus::Failed {
        return crate::workflows::run::summarize(record);
    }
    record
        .summary
        .clone()
        .unwrap_or_else(|| crate::workflows::run::summarize(record))
}
