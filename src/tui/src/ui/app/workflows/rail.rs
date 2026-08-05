//! The catalogue rail: the workflows, and the selected one's runs beneath it.
//!
//! Only the selected workflow's runs are listed. A rail that expanded every
//! workflow would be mostly history — a machine with ten workflows and a nightly
//! schedule has hundreds of runs — and the question the rail answers is "which
//! plan am I looking at", with "and how did it last go" as the follow-up.
//!
//! The cursor is a tree position (a workflow, optionally a run under it) rather
//! than an index into a flattened list, because the run rows only exist while
//! their workflow is selected.

use medulla::ui::workflows::{run_rows, workflow_rows, WorkflowRow};
use medulla::workflows::RunStatus;

use super::super::types::App;

/// What the row that starts a new workflow says.
///
/// Shared with the renderer so the row's width and its text cannot disagree —
/// a sidebar sized to one label and drawn with another clips itself.
pub(in crate::ui::app) const NEW_LABEL: &str = "+ New workflow";

/// One row of the rail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowRailRow {
    /// An installed workflow.
    Workflow {
        /// Its position in the catalogue.
        index: usize,
        /// The listing row.
        row: WorkflowRow,
    },
    /// One run of the selected workflow, indented beneath it.
    Run {
        /// Its position in the run history, newest first.
        index: usize,
        /// Execution state, retained so the renderer can colour the row.
        status: RunStatus,
        /// The listing row.
        row: WorkflowRow,
    },
    /// A non-selectable line under a selected workflow: what it has no history
    /// of, or what it is waiting on.
    ///
    /// Named `Hint` rather than `Note` because a workflow now genuinely has
    /// notes, and a row variant meaning "an explanatory line" beside a domain
    /// type meaning "a durable claim" is a collision waiting to mislead.
    Hint(String),
    /// The row that starts a new workflow: a copilot thread with nothing behind
    /// it yet. Always last, because it is an action rather than an entry and a
    /// catalogue that opened with one reads as though it were empty.
    New,
}

impl App {
    /// The rail's rows, top to bottom.
    pub(in crate::ui::app) fn workflow_rail_rows(&self) -> Vec<WorkflowRailRow> {
        let mut rows = Vec::new();
        for (index, row) in workflow_rows(self.workflow_summaries())
            .into_iter()
            .enumerate()
        {
            let selected = index == self.workflow_index;
            rows.push(WorkflowRailRow::Workflow { index, row });
            if !selected {
                continue;
            }
            if let Some(error) = self.workflow_runs_error() {
                // Distinct from "no runs yet": one of these is fine and the
                // other is something an operator should go and look at.
                let _ = error;
                rows.push(WorkflowRailRow::Hint("run history unreadable".into()));
                continue;
            }
            // Above the runs, because it is the one thing on this workflow
            // waiting on the operator rather than a record of what already
            // happened.
            if self.actionable_proposal().is_some() {
                rows.push(WorkflowRailRow::Hint(
                    "a change is proposed — a to apply, n to decline".into(),
                ));
            }
            let runs = run_rows(self.workflow_runs());
            if runs.is_empty() {
                rows.push(WorkflowRailRow::Hint("no runs yet".into()));
                continue;
            }
            for (index, row) in runs.into_iter().enumerate() {
                let status = self.workflow_runs()[index].status;
                rows.push(WorkflowRailRow::Run { index, status, row });
            }
        }
        rows.push(WorkflowRailRow::New);
        rows
    }

    /// The columns `row` wants, so the sidebar can be sized to its content.
    ///
    /// Counts the marker and the indent as well as the text, because those are
    /// what a row is actually drawn with — sizing to the label alone clips every
    /// row by the width of its own decoration.
    pub(in crate::ui::app) fn workflow_rail_width(&self, row: &WorkflowRailRow) -> usize {
        const MARKER: usize = 1;
        const RUN_INDENT: usize = 2;
        let (text, indent) = match row {
            WorkflowRailRow::Workflow { index, row } => (
                // The jump digit is drawn ahead of the label.
                format!("{} {}", index + 1, row.label),
                0,
            ),
            WorkflowRailRow::Run { .. } => (
                crate::ui::app::render::workflows::rail::rail_label(row),
                RUN_INDENT,
            ),
            WorkflowRailRow::Hint(hint) => (hint.clone(), RUN_INDENT),
            WorkflowRailRow::New => (NEW_LABEL.to_string(), 0),
        };
        MARKER + indent + text.chars().count()
    }

    /// Whether the rail cursor is on `row`.
    pub(in crate::ui::app) fn workflow_rail_selected(&self, row: &WorkflowRailRow) -> bool {
        match row {
            // The New row owns the cursor outright when it has it: no workflow
            // is selected while it does.
            WorkflowRailRow::Workflow { index, .. } => {
                !self.wf.creating && *index == self.workflow_index && self.wf.run_index.is_none()
            }
            WorkflowRailRow::Run { index, .. } => {
                !self.wf.creating && self.wf.run_index == Some(*index)
            }
            WorkflowRailRow::New => self.wf.creating,
            WorkflowRailRow::Hint(_) => false,
        }
    }

    /// Move the rail cursor one row.
    ///
    /// Walks into a workflow's runs and back out of them, so one pair of arrow
    /// keys covers the whole rail — there is no separate "expand" gesture to
    /// learn, and no way to be on a run whose workflow is not selected.
    pub(in crate::ui::app) fn move_workflow_rail(&mut self, up: bool) {
        let runs = self.workflow_runs().len();
        let workflows = self.workflows.len();

        // The New row sits below the whole catalogue, so it is where Down from
        // the bottom lands and Up from it comes back. With no workflows at all
        // it is the only row there is, and the cursor stays on it.
        if self.wf.creating {
            if up && workflows > 0 {
                self.wf.creating = false;
                self.select_workflow(workflows - 1);
                self.wf.run_index = self.workflow_runs().len().checked_sub(1);
                self.sync_workflow_overlay();
            }
            return;
        }
        if workflows == 0 {
            self.wf.creating = !up;
            return;
        }
        // Down from the last row of the last workflow steps onto New.
        let on_last = self.workflow_index + 1 >= workflows
            && self
                .wf
                .run_index
                .map_or(runs == 0, |index| index + 1 >= runs);
        if !up && on_last {
            self.wf.creating = true;
            return;
        }

        match (up, self.wf.run_index) {
            // Up from the first run, or from a workflow with none above it,
            // lands on the workflow itself.
            (true, Some(0)) => self.wf.run_index = None,
            (true, Some(index)) => self.wf.run_index = Some(index - 1),
            (true, None) => {
                if self.workflow_index == 0 {
                    return;
                }
                self.workflow_index -= 1;
                // Entering a workflow from below lands on its last run, which is
                // its oldest — the row physically above the next workflow.
                self.select_workflow(self.workflow_index);
                self.wf.run_index = self.workflow_runs().len().checked_sub(1);
            }
            (false, None) if runs > 0 => self.wf.run_index = Some(0),
            (false, Some(index)) if index + 1 < runs => self.wf.run_index = Some(index + 1),
            (false, _) => {
                if self.workflow_index + 1 >= workflows {
                    return;
                }
                self.select_workflow(self.workflow_index + 1);
            }
        }
        self.wf.preview_scroll = 0;
        self.sync_workflow_overlay();
    }

    /// Open the Workflows tab on `workflow`, overlaid with `run` if it is known.
    ///
    /// Reached from the Agents rail, where a run started over MCP is listed
    /// under the session that started it. The run may not be in the history yet
    /// — it is still executing, and the record is written when it settles — so a
    /// missing one selects the workflow and leaves the overlay for the run rows
    /// to catch up with, rather than refusing the jump.
    pub(in crate::ui::app) fn open_workflow_run(&mut self, workflow: &str, run: &str) {
        self.tab_index = super::super::types::tab_pos("Workflows");
        let Some(index) = self
            .workflows
            .iter()
            .position(|summary| summary.id == workflow)
        else {
            self.set_status(format!("Workflow '{workflow}' is not in this catalogue"));
            return;
        };
        self.select_workflow(index);
        if let Some(position) = self
            .workflow_runs()
            .iter()
            .position(|record| record.id == run)
        {
            self.wf.run_index = Some(position);
            self.wf.overlay = Some(run.to_string());
        } else {
            // The graph still overlays the live run: the node preview reads its
            // streamed output by run id, and that arrives before the record.
            self.wf.overlay = Some(run.to_string());
        }
    }

    /// Select workflow `index`, reloading everything that hangs off it.
    ///
    /// Leaves the New row, since a workflow and "no workflow yet" cannot both
    /// be selected.
    pub(in crate::ui::app) fn select_workflow(&mut self, index: usize) {
        self.wf.creating = false;
        self.workflow_index = index;
        self.wf.run_index = None;
        self.wf.overlay = None;
        self.wf.node_index = 0;
        self.wf.canvas_row = 0;
        self.wf.preview_scroll = 0;
        self.wf.copilot_scroll = 0;
        self.reload_workflow_runs();
        self.reload_workflow_graph();
    }

    /// Point the graph overlay at the run under the cursor, or at nothing.
    fn sync_workflow_overlay(&mut self) {
        self.wf.overlay = self
            .wf
            .run_index
            .and_then(|index| self.workflow_runs().get(index))
            .map(|run| run.id.clone());
    }

    /// The run the graph is currently overlaid with, if any.
    pub(in crate::ui::app) fn selected_workflow_run(
        &self,
    ) -> Option<&medulla::workflows::RunRecord> {
        let id = self.wf.overlay.as_ref()?;
        self.workflow_runs().iter().find(|run| &run.id == id)
    }
}
