//! The Workflows page's state: reading the installed catalog, and running one.
//!
//! The workflows themselves are the SDK's ([`medulla::workflows`]); this is the
//! app-side half — which store this client reads, and what `r` and `Enter` do to
//! it. Reading is cached rather than done per frame, because the store is files
//! and a render pass should not touch the disk.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use medulla::workflows::{WorkflowStore, WorkflowSummary};

use super::types::App;

impl App {
    /// The workflow store this client reads.
    ///
    /// Layered exactly as every other workflow surface resolves it: the
    /// user-global directory, then the project-local one, so a workflow checked
    /// into a repository shadows a personal one of the same id.
    pub(in crate::ui::app) fn workflow_store(&self) -> Arc<dyn WorkflowStore> {
        let env: HashMap<String, String> = std::env::vars().collect();
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        match &self.medulla_home {
            // An injected home (tests, and `main` once it has resolved the real
            // one) must win over the env, or a test would read the developer's
            // own workflows.
            Some(home) => Arc::new(medulla::workflows::FileWorkflowStore::new(
                vec![
                    home.join("workflows"),
                    cwd.join(".medulla").join("workflows"),
                ],
                home.join("state").join("workflows").join("runs"),
            )),
            None => medulla::workflows::discover_store(&env, &cwd),
        }
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
            return;
        };
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
                self.reload_workflow_runs();
                self.set_status(format!(
                    "{count} workflow{} in {}",
                    if count == 1 { "" } else { "s" },
                    self.workflow_dir_label()
                ));
            }
            Err(err) => {
                self.workflows.clear();
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

    /// How many workflows the page is listing. Test/inspection seam.
    pub fn workflow_row_count(&self) -> usize {
        self.workflows.len()
    }
}
