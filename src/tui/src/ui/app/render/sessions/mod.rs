//! The Sessions tab: the sessions surface. The rail is on the left, and the
//! selected row's terminal, diff, run or transcript fills the right.
//!
//! Chat used to live here — selecting the orchestrator's lane and typing *was*
//! the conversation. The orchestrator is a subconscious layer now: it runs below
//! the surface rather than being driven from one, so it has no lane, no
//! transcript and no composer here. What is left is the thing the tab was always
//! also for — the sessions running on this machine and the ones dispatched to
//! it — and every row on it is something already running somewhere.
//!
//! Split by responsibility: [`types`] resolves what the cursor is on and where
//! the panes landed, [`session`] resolves which session that row names and what
//! it arms, [`rail`] draws the list, [`transcript`] the pane beside it,
//! [`summary`] what that pane shows for a row with no transcript of its own,
//! [`run`] the workflow run a row names, and [`work`] the panel showing what the
//! selected session is working on. This module owns only the layout that decides
//! how much room each one gets.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;

use super::super::types::App;

mod harness;
mod rail;
mod run;
mod session;
mod summary;
mod transcript;
mod types;
mod work;

#[cfg(test)]
mod run_tests;
#[cfg(test)]
mod transcript_tests;
#[cfg(test)]
mod work_tests;

use types::{Selection, SessionsPanes};

pub(in crate::ui::app) use rail::RAIL_MAX_CONTENT;

impl App {
    /// Draw the Sessions tab: rail, transcript, composer.
    pub(super) fn draw_sessions_tab(&mut self, f: &mut Frame, area: Rect) {
        let selection = self.sessions_selection();
        let panes = self.sessions_panes(area, &selection);
        for pane in [Some(panes.rail), Some(panes.pane), panes.work]
            .into_iter()
            .flatten()
        {
            self.note_pane(pane);
        }
        self.draw_sessions_rail(f, &panes, &selection);
        self.draw_sessions_pane(f, panes.pane, &selection);
        // Cloned rather than borrowed: the draw takes `&mut self`, and the
        // snapshot lives inside the same state it is drawing from.
        if let (Some(area), Some(snapshot)) = (panes.work, self.selected_work(&selection).cloned())
        {
            self.draw_sessions_work(f, area, &snapshot);
        }
    }

    /// Resolve what the rail cursor is on, clamping it to the rows that exist.
    fn sessions_selection(&mut self) -> Selection {
        let lanes = self.lanes();
        let rows = self.rail_rows_in(&lanes);
        let active = self.rail_cursor_in(&rows, &lanes);
        self.set_rail_cursor_in(&rows, &lanes, active);
        // No fallback: a row with no lane keeps `None`, and the pane renders
        // what that row *is* rather than whoever happens to hold lane 0 — which
        // is the orchestrator, so the old `unwrap_or(0)` put its thinking under
        // `+ New agent`, host headers and idle agents.
        let lane_index = rows.get(active).and_then(|row| row.lane_index());
        let task = rows.get(active).and_then(|row| row.task()).cloned();
        let workflow_run = rows.get(active).and_then(|row| row.workflow_run()).cloned();
        let mut selection = Selection {
            rows,
            active,
            lanes,
            lane_index,
            task,
            session: None,
            workflow_run,
        };
        self.resolve_selected_session(&mut selection);
        selection
    }

    /// Synchronize the workflow canvas after the rail selection or its reports
    /// change. This is an input/state transition, never part of drawing.
    #[cfg(feature = "workflows")]
    pub(in crate::ui::app) fn sync_selected_workflow_run(&mut self) {
        let run = self
            .rail_rows()
            .get(self.rail_index)
            .and_then(|row| row.workflow_run())
            .cloned();
        self.mirror_selected_workflow_run(run.as_ref());
    }

    /// Point the workflow state at the run under the cursor, if it moved.
    ///
    /// The inline run view is the Workflows tab's own canvas, and that canvas
    /// reads the selected workflow and overlay out of [`WorkflowsState`]. So
    /// selecting a run here has to move that state — but only when the selection
    /// actually changed, because doing it is a run-store read and a graph
    /// re-layout, and this runs once per frame.
    ///
    /// Stepping off a run clears the mark rather than the state: the Workflows
    /// tab keeps whatever it was last showing, which is what an operator who
    /// followed a run there and pressed Tab expects to find.
    ///
    /// [`WorkflowsState`]: super::super::types::WorkflowsState
    fn mirror_selected_workflow_run(
        &mut self,
        selected_run: Option<&super::super::rail::WorkflowRunRailRow>,
    ) {
        let Some(run) = selected_run else {
            self.wf.mirrored_run = None;
            self.wf.mirrored_run_updated_at = None;
            return;
        };
        let run = &run.run;
        let synchronized = self.wf.mirrored_run.as_deref() == Some(run.run_id.as_str())
            && self.wf.mirrored_run_updated_at == Some(run.updated_at)
            && self.wf.overlay.as_deref() == Some(run.run_id.as_str())
            && self
                .selected_workflow()
                .is_some_and(|workflow| workflow.id == run.workflow_id);
        if synchronized {
            return;
        }
        self.wf.mirrored_run = Some(run.run_id.clone());
        self.wf.mirrored_run_updated_at = Some(run.updated_at);
        let active_node = run
            .frames
            .iter()
            .rev()
            .find_map(|frame| frame.node.as_deref());
        // A report refreshes the same canvas while an operator may be paging
        // through its live preview. Selecting the workflow reloads the canvas
        // and normally starts that preview at its tail; retain the operator's
        // position only when fresher data still describes that same node.
        let preview_scroll = (self.wf.overlay.as_deref() == Some(run.run_id.as_str())
            && self
                .wf
                .layout
                .nodes
                .get(self.wf.node_index)
                .is_some_and(|node| Some(node.id.as_str()) == active_node))
        .then_some(self.wf.preview_scroll);
        self.point_workflows_at_run(&run.workflow_id, &run.run_id, active_node);
        if let Some(preview_scroll) = preview_scroll {
            self.wf.preview_scroll = preview_scroll;
        }
    }

    /// Divide `area` between the rail, the pane, and the work panel.
    ///
    /// The rail is sized to its widest row rather than to a percentage: its
    /// labels are short and fixed-ish, and the pane is what benefits from
    /// width. It is also capped at [`rail::RAIL_MAX_CONTENT`]: rows that want
    /// more than that wrap, so one harness with a deep working directory can no
    /// longer buy the whole left third of the screen for a path.
    fn sessions_panes(&mut self, area: Rect, selection: &Selection) -> SessionsPanes {
        // Measured at the cap, since that is the width the rows will wrap to.
        let widest = selection
            .rows
            .iter()
            .flat_map(|row| self.rail_row_measurement_lines(row, &selection.lanes))
            .map(|line| line.width())
            // The device footer shares the rail, so a rail sized to its rows
            // alone left a configured bar too narrow to ever render.
            .chain(std::iter::once(crate::ui::resources::device_width_hint(
                &self.loaded.config.appearance,
                self.device_monitor.last(),
            )))
            .max()
            .unwrap_or(0)
            .min(rail::RAIL_MAX_CONTENT);
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(crate::ui::multi_pane::sidebar_width(area.width, widest)),
                Constraint::Min(0),
            ])
            .split(area);
        // A harness owns the whole right column, and so does a workflow run: the
        // graph plus the live output of the step that is working both want the
        // width. The work panel goes for the same reason — the harness's own
        // screen already shows its todos and edits, and the columns are better
        // spent on the terminal than on our second-hand copy of it.
        let embedded = selection.session.is_some() || selection.workflow_run.is_some();
        let show_work = !embedded
            && self.selected_work(selection).is_some()
            && area.width >= work::MIN_WIDTH_FOR_WORK_PANE;
        let (pane, work_area) = if show_work {
            let split = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Min(0),
                    Constraint::Length(work::WORK_PANE_WIDTH),
                ])
                .split(columns[1]);
            (split[0], Some(split[1]))
        } else {
            (columns[1], None)
        };
        SessionsPanes {
            rail: columns[0],
            pane,
            work: work_area,
        }
    }
}
