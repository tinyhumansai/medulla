//! Running an installed workflow in answer to a task frame.
//!
//! An ordinary task frame carries an instruction and this worker hands it to a
//! harness. A frame naming a `workflow` carries an *id*, and this worker runs
//! the saved graph instead — dispatching each of its `agent` nodes to its own
//! harness, in the order and with the parallelism the graph declares.
//!
//! That is the whole of "the orchestrator can execute workflows": one extra
//! field on a frame it already sends, over the transport it already uses. The
//! admission check, the ack, and the reply are the same ones an ordinary task
//! gets, so an orchestrator that knows nothing about workflows still sees a
//! task it dispatched and a task that answered.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::flow_engine::caps::dispatch::HarnessDispatch;
use crate::flow_engine::{folding_sink, CapabilitySettings, HostServices};
use crate::hub::{RunError, TaskOutcome, TaskRequest};
use crate::tinyplace::{TaskFrame, TaskFrameKind, TokenUsage, WorkflowAdvert};
use crate::workflows::evolve::{EvolveConfig, EvolveSession, EvolveTrigger};
use crate::workflows::{
    run_workflow, FileWorkflowStore, RunContext, RunStatus, StoreWorkflowResolver, WorkflowStore,
};

use super::super::providers::{Abort, RunTaskOptions};
use super::super::types::{DaemonRuntime, FrameAttachments};

/// Dispatch a workflow's `agent` nodes through this daemon's own executor.
///
/// A worker running a workflow already *is* a harness host, so a node's
/// instruction goes straight to the executor rather than back out over a bridge
/// to itself. The node's `agent_ref` names a provider hint when it matches one
/// this worker offers; otherwise the worker's default runs it.
struct RuntimeDispatch {
    runtime: DaemonRuntime,
    /// The authenticated sender the workflow is being run for, so nodes inherit
    /// the same conversation attribution an ordinary task would get.
    conversation: String,
}

#[async_trait]
impl HarnessDispatch for RuntimeDispatch {
    async fn dispatch(&self, request: TaskRequest) -> Result<TaskOutcome, RunError> {
        let inner = &self.runtime.inner;
        // A node may name a provider through its `agent_ref`; anything this
        // worker does not offer falls back to the default rather than failing,
        // because a graph should be portable across workers.
        let provider = crate::tinyplace::HarnessProvider::from_wire(&request.worker_address)
            .filter(|p| inner.config.providers.contains(p))
            .or_else(|| self.runtime.select_provider(request.provider))
            .unwrap_or(inner.config.default_provider);

        let options = RunTaskOptions {
            conversation: self.conversation.clone(),
            // A workflow node is discrete work, like the task frame that
            // started the graph — nodes share a conversation for attribution,
            // not a harness. Two nodes of one graph running in the same session
            // would let a later node read an earlier one's prompt as context.
            session_class: crate::sessions::SessionClass::Bounded,
            resume_session_id: None,
            provider,
            prompt: request.instruction,
            cwd: inner.config.workspace.clone(),
            env: inner.config.env.clone(),
            timeout_ms: inner.config.task_timeout_ms,
            model: request.model.or_else(|| inner.config.model.clone()),
            agent: inner.config.agent.clone(),
            extra_args: inner.config.extra_args.clone(),
            skip_permissions: inner.config.skip_permissions,
            router: inner.config.router.clone(),
            abort: Abort::new(),
            // The run observer already reports progress per node; forwarding a
            // harness's token-level chatter as well would double-report it.
            on_event: None,
            on_stdin: None,
            on_session: None,
        };

        let result = (inner.run_task)(options).await.map_err(RunError::Worker)?;
        Ok(TaskOutcome {
            reply: result.reply,
            usage: result.usage.unwrap_or(TokenUsage {
                input_tokens: 0,
                output_tokens: 0,
            }),
            harness: Some(provider),
        })
    }
}

impl DaemonRuntime {
    /// The workflows this worker has installed, for its capability advert.
    ///
    /// Best effort: a store that cannot be read advertises nothing rather than
    /// failing the probe, because a worker with an unreadable workflow directory
    /// is still a perfectly good worker for ordinary tasks.
    pub(super) fn installed_workflows(&self) -> Vec<WorkflowAdvert> {
        // A host with workflows off advertises none: an orchestrator should not
        // be told about work it will be refused.
        if !self.workflow_settings().enabled {
            return Vec::new();
        }
        self.workflow_store()
            .list()
            .unwrap_or_default()
            .into_iter()
            .map(|summary| WorkflowAdvert {
                id: summary.id,
                name: summary.name,
                description: summary.description,
                node_count: summary.node_count,
            })
            .collect()
    }

    /// The workflow store for this worker: its configured workspace, layered
    /// over the user-global directory the way every other workflow surface
    /// resolves it.
    fn workflow_store(&self) -> Arc<dyn WorkflowStore> {
        let cwd = std::path::Path::new(&self.inner.config.workspace);
        Arc::new(FileWorkflowStore::discover(&self.inner.config.env, cwd))
    }

    /// Run the workflow a `task` frame named, replying with its outcome.
    pub(super) async fn handle_workflow_task(&self, from: String, frame: TaskFrame, id: String) {
        let correlation = frame.correlation_id.clone();

        let settings = self.workflow_settings();
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

        if self.inner.admitted.load(Ordering::SeqCst) >= self.inner.config.max_pending {
            self.reply(
                &from,
                TaskFrameKind::Error,
                &frame.task_id,
                &format!(
                    "daemon at capacity ({} pending tasks); retry later",
                    self.inner.config.max_pending
                ),
                correlation.as_deref(),
                None,
            )
            .await;
            return;
        }

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

        self.inner.admitted.fetch_add(1, Ordering::SeqCst);
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
        let context = RunContext {
            store: store.clone(),
            settings,
            services: HostServices {
                dispatch: Arc::new(RuntimeDispatch {
                    runtime: self.clone(),
                    conversation: from.clone(),
                }),
                resolver: Arc::new(StoreWorkflowResolver::new(store)),
                http_credentials: Default::default(),
            },
            sink,
        };

        // The frame's task id becomes the run id, so the orchestrator's existing
        // `abort` for that task is exactly what cancels the run.
        let outcome = run_workflow(context, &id, &frame.task_id, trigger_input(&frame.text)).await;
        self.inner.admitted.fetch_sub(1, Ordering::SeqCst);

        let work = fold.lock().ok().map(|fold| fold.snapshot().clone());
        let attachments = FrameAttachments { usage: None, work };

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
            dispatch: Arc::new(RuntimeDispatch {
                runtime: self.clone(),
                conversation: from.to_string(),
            }),
            worker_address: self.inner.config.default_provider.as_str().to_string(),
            provider: Some(self.inner.config.default_provider),
            model: self.inner.config.model.clone(),
            // Per workflow, not per run: successive reviews of the same
            // workflow are the same conversation, which is what lets one build
            // on what the last concluded.
            conversation: format!("evolve:{workflow_id}"),
            config: settings,
        };
        let workflow_id = workflow_id.to_string();
        let trigger = EvolveTrigger::Failure(run_id.to_string());
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
        });
    }

    /// This worker's review settings, defaulting safely when config is
    /// unreadable.
    fn evolve_settings(&self) -> EvolveConfig {
        let cwd = std::path::Path::new(&self.inner.config.workspace);
        crate::config::load_config(None, &self.inner.config.env, cwd)
            .map(|loaded| EvolveConfig::from_config(&loaded.config.workflows))
            .unwrap_or_default()
    }

    /// Capability settings for workflows run on this worker.
    ///
    /// Read from the operator's layered config so `workflows.enabled` and the
    /// allowlists mean the same thing here as they do on the CLI. A config that
    /// cannot be loaded falls back to the safe defaults — no code execution, no
    /// outbound HTTP, no third-party tools — rather than to permissive ones.
    fn workflow_settings(&self) -> Arc<CapabilitySettings> {
        let home = crate::home::medulla_home(&self.inner.config.env);
        let cwd = std::path::Path::new(&self.inner.config.workspace);
        let mut settings = crate::config::load_config(None, &self.inner.config.env, cwd)
            .map(|loaded| CapabilitySettings::from_config(&loaded.config.workflows, &home))
            .unwrap_or_else(|_| CapabilitySettings::rooted_at(home));
        // The daemon's own workspace, which is the directory it serves tasks
        // for — the same one an `agent` node's harness session runs in.
        settings.workspace = self.inner.config.workspace.clone();
        settings.default_worker_address = self.inner.config.default_provider.as_str().to_string();
        settings.default_provider = Some(self.inner.config.default_provider);
        settings.default_model = self.inner.config.model.clone();
        Arc::new(settings)
    }
}

/// The trigger payload for a workflow run, from a frame's text.
///
/// JSON when the orchestrator sent JSON, and `{ "text": … }` otherwise — so a
/// frame carrying an ordinary instruction still gives the graph something its
/// expressions can read.
fn trigger_input(text: &str) -> Value {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Value::Object(Default::default());
    }
    serde_json::from_str(trimmed).unwrap_or_else(|_| serde_json::json!({ "text": text }))
}

/// A one-line account of how a run ended, for the reply frame's text.
///
/// Delegates rather than phrasing its own: a run that is described one way in
/// the reply frame and another way in its own record is a run an operator has
/// to reconcile by hand.
fn summarize(record: &crate::workflows::RunRecord) -> String {
    record
        .summary
        .clone()
        .unwrap_or_else(|| crate::workflows::run::summarize(record))
}
