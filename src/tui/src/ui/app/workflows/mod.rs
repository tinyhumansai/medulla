//! The Workflows tab's state: the catalogue, the graph, and the copilot thread.
//!
//! The workflows themselves are the SDK's ([`medulla::workflows`]), and so is
//! every judgement about how they are laid out and described
//! ([`medulla::ui::workflows`]). This is the app-side half: which store this
//! client reads, which of the three panes has the cursor, and what each pane's
//! cursor is on.
//!
//! Everything is cached rather than read per frame — the store is files, and a
//! render pass must not touch the disk. The cache is refreshed when the
//! selection changes, when `r` is pressed, and after anything that could have
//! written to the store (a run, a copilot turn).
//!
//! - [`live`] — what a run in flight is doing, per node, while it runs.
//! - [`rail`] — the catalogue cursor, and the runs nested under it.
//! - [`canvas`] — the graph cache and the node cursor over it.
//! - [`copilot`] — the per-workflow conversation and its turns.

mod canvas;
pub(super) use canvas::node_label;
mod copilot;
mod live;
pub use copilot::thread_of as copilot_thread_of;
mod rail;

pub(in crate::ui::app) use live::LiveRun;
pub(in crate::ui::app) use rail::{WorkflowRailRow, NEW_LABEL};

#[cfg(test)]
mod copilot_overlap_tests;
#[cfg(test)]
mod proposal_tests;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use medulla::workflows::{WorkflowStore, WorkflowSummary};

use super::types::App;

impl App {
    /// The workflow store this client reads.
    ///
    /// Layered exactly as every other workflow surface resolves it: checked-in
    /// project defaults first, then the user-global directory where authored
    /// workflows are saved.
    pub(in crate::ui::app) fn workflow_store(&self) -> Arc<dyn WorkflowStore> {
        if let Some(store) = &self.workflow_store_override {
            return store.clone();
        }
        let env: HashMap<String, String> = std::env::vars().collect();
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        match &self.medulla_home {
            // An injected home (tests, and `main` once it has resolved the real
            // one) must win over the env, or a test would read the developer's
            // own workflows.
            Some(home) => Arc::new(medulla::workflows::FileWorkflowStore::with_workspace_state(
                vec![
                    cwd.join(".medulla").join("workflows"),
                    home.join("workflows"),
                ],
                &home.join("state").join("workflows"),
                &cwd,
            )),
            None => medulla::workflows::discover_store(&env, &cwd),
        }
    }

    /// Read and write workflows through `store` rather than the layered one.
    ///
    /// The seam a test uses to get a catalogue that is only what it installed;
    /// see [`App::workflow_store_override`](super::types::App).
    pub fn set_workflow_store(&mut self, store: Arc<dyn WorkflowStore>) {
        self.workflow_store_override = Some(store);
    }

    /// The workflows currently listed on the page.
    pub fn workflow_summaries(&self) -> &[WorkflowSummary] {
        &self.workflows
    }

    /// The selected workflow's runs, as last read.
    pub(in crate::ui::app) fn workflow_runs(&self) -> &[medulla::workflows::RunRecord] {
        &self.workflow_runs
    }

    /// Why the run history could not be read, if it could not.
    ///
    /// Kept so the page can say "unreadable" rather than silently showing the
    /// same thing it shows for "never run" — those are different problems and
    /// only one of them is fine.
    pub(in crate::ui::app) fn workflow_runs_error(&self) -> Option<&str> {
        self.workflow_runs_error.as_deref()
    }

    /// Re-read the selected workflow's run history.
    ///
    /// Called when the selection or the catalogue changes, never from a render
    /// pass: drawing a frame must not touch the disk.
    pub(in crate::ui::app) fn reload_workflow_runs(&mut self) {
        let Some(workflow) = self.selected_workflow().map(|w| w.id.clone()) else {
            self.workflow_runs.clear();
            self.workflow_runs_error = None;
            self.workflow_notes.clear();
            self.workflow_proposals.clear();
            return;
        };
        // Both read as "nothing learned" when the store cannot answer. Unlike
        // the run history there is no error line for these: a journal that
        // cannot be read is already logged where it is read, and a second
        // error banner for something passive would crowd out the run's.
        self.workflow_notes = self
            .workflow_store()
            .list_notes(&workflow)
            .unwrap_or_default();
        self.workflow_proposals = self
            .workflow_store()
            .list_proposals(&workflow)
            .unwrap_or_default();
        match self.workflow_store().list_runs(&workflow) {
            Ok(runs) => {
                self.workflow_runs = runs;
                self.workflow_runs_error = None;
            }
            Err(err) => {
                self.workflow_runs.clear();
                self.workflow_runs_error = Some(err.to_string());
            }
        }
        // A run that has gone (a pruned history, a different workflow) must not
        // leave the cursor pointing past the end of the list.
        let count = self.workflow_runs.len();
        if self.wf.run_index.is_some_and(|index| index >= count) {
            self.wf.run_index = None;
            self.wf.overlay = None;
        }
    }

    /// What the selected workflow has learned, newest first.
    pub(in crate::ui::app) fn workflow_notes(&self) -> &[medulla::workflows::WorkflowNote] {
        &self.workflow_notes
    }

    /// The one proposal an operator could act on right now.
    ///
    /// At most one exists: a review supersedes the workflow's other undecided
    /// proposals as it stores its own, which is what lets the accept and reject
    /// keys work without a cursor to pick between them.
    pub(in crate::ui::app) fn actionable_proposal(
        &self,
    ) -> Option<&medulla::workflows::WorkflowProposal> {
        medulla::ui::workflows::actionable(&self.workflow_proposals)
    }

    /// The newest pending proposal, including one whose checks failed.
    pub(in crate::ui::app) fn visible_proposal(
        &self,
    ) -> Option<&medulla::workflows::WorkflowProposal> {
        medulla::ui::workflows::displayed(&self.workflow_proposals)
    }

    /// Re-read the workflow store into the page.
    ///
    /// An unreadable store leaves the page empty and says so, rather than
    /// failing: the usual way a workflow changes is the operator's own editor,
    /// and a half-written file should cost a refresh, not the app.
    pub fn reload_workflows(&mut self) {
        match self.workflow_store().list() {
            Ok(workflows) => {
                let count = workflows.len();
                self.workflows = workflows;
                self.workflow_index = self.workflow_index.min(count.saturating_sub(1));
                // With nothing installed, the New row is not *a* row — it is
                // the only one, and the rail draws it as though the cursor were
                // on it. Leaving `creating` false made that a lie the copilot
                // then acted on: pressing `c` on the one visible row answered
                // "select a workflow first", which on an empty host is advice
                // that cannot be followed and reads as the copilot refusing to
                // work at all.
                if count == 0 {
                    self.wf.creating = true;
                }
                self.reload_workflow_runs();
                self.reload_workflow_graph();
                self.set_status(format!(
                    "{count} workflow{} in {}",
                    if count == 1 { "" } else { "s" },
                    self.workflow_dir_label()
                ));
            }
            Err(err) => {
                self.workflows.clear();
                self.wf.graph = None;
                self.wf.layout = Default::default();
                self.set_status(format!("Cannot read workflows: {err}"));
            }
        }
    }

    /// A short label naming where workflows are read from, for the status line.
    fn workflow_dir_label(&self) -> String {
        match &self.medulla_home {
            Some(home) => home.join("workflows").display().to_string(),
            None => ".medulla/workflows".to_string(),
        }
    }

    /// The workflow under the cursor, if the page has any.
    pub(in crate::ui::app) fn selected_workflow(&self) -> Option<&WorkflowSummary> {
        self.workflows.get(self.workflow_index)
    }

    /// The run under the cursor, when the cursor is on one of a workflow's runs
    /// rather than on the workflow itself.
    pub(in crate::ui::app) fn selected_run(&self) -> Option<&medulla::workflows::RunRecord> {
        self.workflow_runs().get(self.wf.run_index?)
    }

    /// Which of the tab's three panes has the keyboard. Test seam.
    #[cfg(test)]
    pub(in crate::ui::app) fn wf_focus(&self) -> super::types::WorkflowFocus {
        self.wf.focus
    }

    /// Whether the node inspector is expanded. Test seam.
    #[cfg(test)]
    pub(in crate::ui::app) fn wf_inspector_open(&self) -> bool {
        self.wf.inspector_open
    }

    /// Which workflow the rail cursor is on. Test/inspection seam.
    pub fn selected_workflow_index(&self) -> usize {
        self.workflow_index
    }

    /// How many workflows the page is listing. Test/inspection seam.
    pub fn workflow_row_count(&self) -> usize {
        self.workflows.len()
    }
}
