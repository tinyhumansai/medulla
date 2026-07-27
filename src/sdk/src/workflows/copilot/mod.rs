//! The workflow copilot: a harness turn scoped to one graph.
//!
//! The Workflows tab shows a graph and a chat beside it. The chat is not a new
//! kind of agent — it is one dispatched harness task per instruction, run
//! against a session that already has the `medulla-workflows` MCP tools
//! ([`crate::workflows::mcp`]) attached. So the copilot's "tools to the graph"
//! are the same operations the `medulla workflow` subcommand and every other
//! authoring surface call, and none of the three can drift from the others.
//!
//! What this module adds on top of a bare dispatch is the three things that make
//! it a copilot rather than a prompt box:
//!
//! - **Scope.** [`prompt`] states which workflow the turn may touch and that
//!   edits go through the tools.
//! - **Progress.** The harness's status frames are forwarded as they arrive, so
//!   a turn that takes a minute is not a frozen pane.
//! - **Accountability.** The graph is snapshotted before and after and the
//!   [`diff`] between them is reported — what the store actually holds, not what
//!   the agent said it did.

mod diff;
mod prompt;

pub use diff::describe as describe_changes;
pub use prompt::build as build_prompt;

use std::sync::Arc;

use tinyflows::model::WorkflowGraph;
use tokio::sync::mpsc;

use crate::flow_engine::caps::dispatch::HarnessDispatch;
use crate::hub::TaskRequest;
use crate::workflows::{require, WorkflowError, WorkflowStore};

/// How one copilot turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopilotOutcome {
    /// The agent's final reply.
    pub reply: String,
    /// What the turn changed in the stored graph, as transcript lines. Empty
    /// when it changed nothing — which is the right outcome for a question, and
    /// a fact worth reporting for anything else.
    pub changes: Vec<String>,
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
    pub provider: Option<crate::tinyplace::HarnessProvider>,
    /// Optional model hint.
    pub model: Option<String>,
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
        let record = require(self.store.as_ref(), workflow_id)?;
        let before = record.graph.clone();
        let prompt = prompt::build(&record.id, &record.name, &before, instruction);

        let task_id = format!("copilot-{}", uuid::Uuid::new_v4());
        let request = TaskRequest {
            task_id: task_id.clone(),
            abort_id: task_id,
            cycle_id: None,
            instruction: prompt,
            worker_address: self.worker_address.clone(),
            provider: self.provider,
            model: self.model.clone(),
            // Never a workflow: this dispatch is an *authoring* turn, and
            // setting this would run the graph the operator is trying to edit.
            workflow: None,
        };

        let outcome = self
            .dispatch
            .dispatch_with_status(request, status)
            .await
            .map_err(|err| WorkflowError::Engine(err.to_string()))?;

        // Re-read rather than trusting the reply: the tools wrote to the store,
        // and the store is the only thing that knows what they wrote.
        let after = self.graph_now(workflow_id, &before);
        Ok(CopilotOutcome {
            reply: outcome.reply.trim().to_string(),
            changes: diff::describe(&before, &after),
        })
    }

    /// The workflow's graph as the store now holds it.
    ///
    /// Falls back to the pre-turn graph when the record has gone — a workflow
    /// the agent deleted, or a store that briefly cannot be read. Reporting "no
    /// changes" there is wrong but harmless; the catalogue refresh that follows
    /// the turn is what shows the deletion.
    fn graph_now(&self, workflow_id: &str, fallback: &WorkflowGraph) -> WorkflowGraph {
        match self.store.get(workflow_id) {
            Ok(Some(record)) => record.graph,
            _ => fallback.clone(),
        }
    }
}

#[cfg(test)]
mod tests;
