//! The Agents tab: the conversation surface. The rail is on the left, the
//! selected row's transcript or declaration fills the right, and the composer
//! sits under it.
//!
//! This is where Chat went. Selecting the orchestrator lane and typing *is* the
//! conversation, so its transcript is the chat log rather than a list of
//! inference turns; selecting an agent shows that agent's own turns, and the
//! composer answers its open question instead of starting a new cycle. One
//! surface, because reading what an operation is doing and steering it were
//! never two jobs.
//!
//! Split by responsibility: [`types`] resolves what the cursor is on and where
//! the panes landed, [`session`] resolves which session that row names and what
//! it arms, [`rail`] draws the threads strip and the lane/fleet list,
//! [`transcript`] the pane beside it, [`summary`] what that pane shows for a row
//! with no transcript of its own, [`run`] the workflow run a row names,
//! [`started`] the sessions each conversation turn spawned, [`work`] the panel
//! showing what the selected agent is working on, and [`composer`] the input
//! under that. This module owns only the layout that decides how much room each
//! one gets.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;

use crate::ui::agents::{AgentRole, AgentRow};

use super::super::rail::RailRow;
use super::super::types::App;

mod composer;
mod harness;
mod rail;
mod run;
mod session;
mod started;
mod summary;
mod transcript;
mod types;
mod work;

#[cfg(test)]
mod run_tests;
#[cfg(test)]
mod started_tests;
#[cfg(test)]
mod transcript_tests;
#[cfg(test)]
mod work_tests;

use types::{AgentsPanes, Selection};

pub(in crate::ui::app) use rail::RAIL_MAX_CONTENT;

impl App {
    /// Draw the Agents tab: rail, transcript, composer.
    pub(super) fn draw_agents(&mut self, f: &mut Frame, area: Rect) {
        let selection = self.agents_selection();
        let panes = self.agents_panes(area, &selection);
        for pane in [
            Some(panes.threads),
            Some(panes.rail),
            Some(panes.pane),
            panes.work,
            Some(panes.composer),
        ]
        .into_iter()
        .flatten()
        {
            self.note_pane(pane);
        }
        self.draw_agents_rail(f, &panes, &selection);
        self.draw_agents_pane(f, panes.pane, &selection);
        // Cloned rather than borrowed: the draw takes `&mut self`, and the
        // snapshot lives inside the same state it is drawing from.
        if let (Some(area), Some(snapshot)) = (panes.work, self.selected_work(&selection).cloned())
        {
            self.draw_agents_work(f, area, &snapshot);
        }
        // What the composer submits depends on the cursor, so it says so. Not
        // drawn at all when a harness has the pane: the layout gave it no rows,
        // and a zero-height composer would still paint its border into the
        // terminal's bottom line.
        if panes.composer.height > 0 {
            self.draw_agent_composer(f, panes.composer, &selection);
        }
    }

    /// Resolve what the rail cursor is on, clamping it to the rows that exist.
    fn agents_selection(&mut self) -> Selection {
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
        // Only a *lane* row can be the orchestrator's. Agents, sessions, hosts
        // and the action rows are not lanes, and reading the role off the old
        // lane-0 fallback claimed the orchestrator's composer for rows that are
        // not it — a text box under a row with nothing to say to. Mirrors
        // [`App::on_orchestrator_lane`](crate::ui::app), which the keyboard
        // reads, so focus and layout cannot disagree.
        let on_orchestrator = match rows.get(active) {
            Some(RailRow::Lane(AgentRow::Lane { .. })) => lane_index
                .and_then(|index| lanes.get(index))
                .map(|l| l.role == AgentRole::Orchestrator)
                .unwrap_or(true),
            None => true,
            Some(_) => false,
        };
        let workflow_run = rows.get(active).and_then(|row| row.workflow_run()).cloned();
        let mut selection = Selection {
            rows,
            active,
            lanes,
            lane_index,
            task,
            on_orchestrator,
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
            .get(self.agent_index)
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

    /// Divide `area` between the rail, the pane, and the composer.
    ///
    /// The rail is sized to its widest row rather than to a percentage: its
    /// labels are short and fixed-ish, and the transcript is what benefits from
    /// width. It is also capped at [`rail::RAIL_MAX_CONTENT`]: rows that want
    /// more than that wrap, so one harness with a deep working directory can no
    /// longer buy the whole left third of the screen for a path. The threads
    /// strip takes rows only when more than one thread is open — with a single
    /// thread the pane title already names it.
    fn agents_panes(&mut self, area: Rect, selection: &Selection) -> AgentsPanes {
        let threads = self.thread_rows();
        // Measured at the cap, since that is the width the rows will wrap to.
        let widest = selection
            .rows
            .iter()
            .flat_map(|row| self.rail_row_measurement_lines(row, &selection.lanes))
            .map(|line| line.width())
            .chain(threads.iter().map(|line| line.width()))
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
        let left = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(if threads.len() > 1 {
                    (threads.len() as u16 + 2).min(area.height / 3)
                } else {
                    0
                }),
                Constraint::Min(0),
            ])
            .split(columns[0]);
        // A harness owns the whole right column. It paints its own composer —
        // showing ours under it would be two input boxes, only one of which the
        // keyboard reaches, and the operator cannot tell which by looking. The
        // work panel goes for the same reason: the harness's own screen already
        // shows its todos and edits, and the columns are better spent on the
        // terminal than on our second-hand copy of it.
        // A workflow run takes the column on the same terms as a harness. It is
        // the graph plus the live output of the step that is working, and both
        // want the width; there is also nothing to type at, since the run is
        // executing in another process entirely.
        let embedded = selection.session.is_some() || selection.workflow_run.is_some();
        // Starting work remains available alongside the fleet; only a live
        // terminal or workflow run takes the entire column.
        let show_composer = !embedded;
        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(if show_composer {
                    self.composer_height(columns[1].width)
                } else {
                    0
                }),
            ])
            .split(columns[1]);
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
                .split(right[0]);
            (split[0], Some(split[1]))
        } else {
            (right[0], None)
        };
        AgentsPanes {
            threads: left[0],
            rail: left[1],
            pane,
            work: work_area,
            composer: right[1],
        }
    }
}
