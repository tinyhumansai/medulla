//! The workflow copilot: a harness turn scoped to one graph.
//!
//! The Workflows tab shows a graph and a chat beside it. The chat is not a new
//! kind of agent — it is one dispatched harness task per instruction, run
//! against a session that already has the `medulla-workflows` MCP tools
//! ([`crate::mcp`]) attached. So the copilot's "tools to the graph"
//! are the same operations the `medulla workflow` subcommand and every other
//! authoring surface call, and none of the three can drift from the others.
//!
//! What this module adds on top of a bare dispatch is the three things that make
//! it a copilot rather than a prompt box:
//!
//! - **Scope.** [`brief`] states which kind of turn this is, which workflow it
//!   may touch, and — in `prompt.md` beside it — the rules every turn runs
//!   under.
//! - **Progress.** The harness's status frames are forwarded as they arrive, so
//!   a turn that takes a minute is not a frozen pane.
//! - **Accountability.** The graph is snapshotted before and after and the
//!   `diff` between them is reported — what the store actually holds, not what
//!   the agent said it did.

mod brief;
mod diff;

pub use brief::{CopilotRequest, FailedRun, Mode};
pub use diff::describe as describe_changes;

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::flow_engine::caps::dispatch::HarnessDispatch;
use crate::hub::TaskRequest;
use crate::workflows::{require, WorkflowError, WorkflowStore};

/// How one copilot turn ended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CopilotOutcome {
    /// The agent's final reply.
    pub reply: String,
    /// What the turn changed in the stored graph, as transcript lines. Empty
    /// when it changed nothing — which is the right outcome for a question, and
    /// a fact worth reporting for anything else.
    pub changes: Vec<String>,
    /// The id of a workflow this turn brought into existence, for a [`create`]
    /// turn that succeeded.
    ///
    /// Always `None` for [`CopilotSession::turn`], which edits a workflow that
    /// already exists. Read from the store rather than from the reply: the
    /// caller uses it to select the new workflow, and selecting one the agent
    /// merely *said* it made would land on nothing.
    ///
    /// [`create`]: CopilotSession::create
    pub created: Option<String>,
    /// Whether the workflow this turn was scoped to no longer exists.
    ///
    /// Reported as a flag rather than left for the caller to spot in
    /// [`changes`](Self::changes): a caller that has to sniff a rendered
    /// transcript line to learn a fact is one refactor away from missing it,
    /// and what hangs on this is releasing the resources the workflow held.
    pub removed: bool,
}

/// Everything a turn needs to reach a harness.
pub struct CopilotSession {
    /// The store the workflow is read from and written back to.
    pub store: Arc<dyn WorkflowStore>,
    /// Where the instruction is run.
    pub dispatch: Arc<dyn HarnessDispatch>,
    /// The bridge address of the worker that runs it.
    pub worker_address: String,
    /// Optional harness hint (`claude`/`codex`/`opencode`).
    pub provider: Option<crate::protocol::HarnessProvider>,
    /// Optional model hint.
    pub model: Option<String>,
    /// The continuity group this pane's turns share.
    ///
    /// The whole reason the copilot reads as a chat rather than a series of
    /// unrelated requests. Without it "now do the same to the other node" could
    /// not work: every turn would be a fresh harness session that had never seen
    /// the last instruction, and the operator would have to restate the context
    /// every time.
    ///
    /// One key per pane, so two workflows being edited side by side keep
    /// separate conversations — which is what the operator means by having two
    /// panes.
    pub conversation: String,
}

impl CopilotSession {
    /// Run one instruction against `workflow_id`.
    ///
    /// `status` receives the harness's progress frames as they arrive; drop the
    /// receiver to stop caring about them, which the sender treats as normal
    /// rather than an error.
    ///
    /// # Errors
    ///
    /// Fails when the workflow is not in the store, or when the dispatch itself
    /// fails — a timeout, an abort, a harness that errored. A turn whose *edit*
    /// was refused is not an error here: the agent will have been told why by
    /// the tool and says so in its reply.
    pub async fn turn(
        &self,
        workflow_id: &str,
        instruction: &str,
        status: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<CopilotOutcome, WorkflowError> {
        self.edit(Mode::Revise, workflow_id, instruction, None, status)
            .await
    }

    /// Run a turn that diagnoses `run` and fixes what caused it.
    ///
    /// The same dispatch as [`Self::turn`], told what it is looking at. The run
    /// is carried in rather than left for the agent to find, because the caller
    /// already has it — the operator pressed a key beside a failure on screen —
    /// and a turn that has to rediscover that starts a step behind.
    ///
    /// # Errors
    ///
    /// As [`Self::turn`].
    pub async fn repair(
        &self,
        workflow_id: &str,
        instruction: &str,
        run: FailedRun,
        status: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<CopilotOutcome, WorkflowError> {
        self.edit(Mode::Repair, workflow_id, instruction, Some(run), status)
            .await
    }

    /// The shared body of every turn scoped to a workflow that already exists.
    async fn edit(
        &self,
        mode: Mode,
        workflow_id: &str,
        instruction: &str,
        run: Option<FailedRun>,
        status: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<CopilotOutcome, WorkflowError> {
        let before = require(self.store.as_ref(), workflow_id)?;
        let prompt = CopilotRequest {
            mode,
            instruction,
            record: Some(&before),
            run,
            notes: &[],
            runs: &[],
        }
        .render();

        let task_id = format!("copilot-{}", uuid::Uuid::new_v4());
        let request = TaskRequest {
            task_id: task_id.clone(),
            abort_id: task_id,
            cycle_id: None,
            instruction: prompt,
            worker_address: self.worker_address.clone(),
            provider: self.provider,
            custom_harness: None,
            model: self.model.clone(),
            // Never a workflow: this dispatch is an *authoring* turn, and
            // setting this would run the graph the operator is trying to edit.
            tool_mode: None,
            workflow: None,
            workflow_fingerprint: None,
            workflow_inputs: Default::default(),
            conversation: Some(self.conversation.clone()),
            fleet_depth: 0,
        };

        let outcome = self.dispatch.dispatch_with_status(request, status).await?;

        // Re-read rather than trusting the reply: the tools wrote to the store,
        // and the store is the only thing that knows what they wrote.
        let (changes, removed) = match self.store.get(workflow_id) {
            Ok(Some(after)) => (diff::describe(&before, &after), false),
            // Gone. Reported explicitly rather than as "no changes": the caller
            // only refreshes the catalogue when something changed, so a silent
            // deletion would leave the workflow on screen after it stopped
            // existing.
            Ok(None) => (vec![format!("− workflow {workflow_id}")], true),
            Err(err) => return Err(err),
        };
        Ok(CopilotOutcome {
            reply: outcome.reply.trim().to_string(),
            changes,
            created: None,
            removed,
        })
    }

    /// Run one instruction that is meant to *create* a workflow.
    ///
    /// The same dispatch as [`Self::turn`] with two differences: the prompt has
    /// no workflow to name and tells the agent to call `workflow_create`, and
    /// what changed is worked out by comparing the catalogue before and after
    /// rather than one graph against itself.
    ///
    /// Reports the new workflow's id in [`CopilotOutcome::created`] so the
    /// caller can select it. A turn where the agent talked but created nothing
    /// is not an error — that is the right outcome for "what could I build?" —
    /// and comes back with `created: None` and no changes.
    ///
    /// # Errors
    ///
    /// Fails when the dispatch itself fails: a timeout, an abort, a harness
    /// that errored. A refused or invalid `workflow_create` is not an error
    /// here; the tool will have told the agent why and it says so in its reply.
    pub async fn create(
        &self,
        instruction: &str,
        status: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<CopilotOutcome, WorkflowError> {
        let before = self.catalogue()?;

        let task_id = format!("copilot-new-{}", uuid::Uuid::new_v4());
        let request = TaskRequest {
            task_id: task_id.clone(),
            abort_id: task_id,
            cycle_id: None,
            instruction: CopilotRequest {
                mode: Mode::Create,
                instruction,
                // Nothing to show: the workflow does not exist yet, and naming
                // an existing one is how an agent talks itself into editing it.
                record: None,
                run: None,
                notes: &[],
                runs: &[],
            }
            .render(),
            worker_address: self.worker_address.clone(),
            provider: self.provider,
            custom_harness: None,
            model: self.model.clone(),
            // Never a workflow, for the same reason an edit is not: this is an
            // authoring turn, not a run.
            tool_mode: None,
            workflow: None,
            workflow_fingerprint: None,
            workflow_inputs: Default::default(),
            conversation: Some(self.conversation.clone()),
            fleet_depth: 0,
        };

        let outcome = self.dispatch.dispatch_with_status(request, status).await?;

        // Whatever is in the catalogue now that was not before. A turn that
        // somehow made several reports all of them — hiding the extras would
        // leave workflows an operator never asked for and cannot see they got —
        // and offers the first by id as the one to select, so the result is
        // stable.
        let created: std::collections::BTreeSet<String> = self
            .catalogue()?
            .into_iter()
            .filter(|id| !before.contains(id))
            .collect();
        Ok(CopilotOutcome {
            reply: outcome.reply.trim().to_string(),
            changes: created
                .iter()
                .map(|id| format!("+ workflow {id}"))
                .collect(),
            created: created.into_iter().next(),
            // A create turn is scoped to no existing workflow, so there is
            // nothing it could have removed.
            removed: false,
        })
    }

    /// Every workflow id the store currently holds.
    ///
    /// Fails rather than yielding nothing on an unreadable store. A silent empty
    /// set here is worse than an error: taken before the turn it makes every
    /// existing workflow look newly created, and `created` then names one the
    /// operator already had — which the caller selects.
    fn catalogue(&self) -> Result<std::collections::BTreeSet<String>, WorkflowError> {
        Ok(self.store.list()?.into_iter().map(|row| row.id).collect())
    }
}

// Shared fixtures for `tests` and `create_tests`, so a stand-in harness or
// store is defined once rather than duplicated across the split.
#[cfg(test)]
mod support;
#[cfg(test)]
mod tests;
// Split out of `tests` once the create-turn cases pushed it over the
// repository's 500-line file ceiling — see that module's doc comment.
#[cfg(test)]
mod create_tests;
