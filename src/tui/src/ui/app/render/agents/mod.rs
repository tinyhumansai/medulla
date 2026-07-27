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
//! the panes landed, [`rail`] draws the threads strip and the lane/fleet list,
//! [`transcript`] the pane beside it, [`work`] the panel showing what the
//! selected agent is working on, and [`composer`] the input under that. This
//! module owns only the layout that decides how much room each one gets.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;

use crate::ui::agents::{AgentRole, AgentRow};

use super::super::rail::RailRow;
use super::super::types::App;

mod composer;
mod rail;
mod transcript;
mod types;
mod work;

#[cfg(test)]
mod work_tests;

use types::{AgentsPanes, Selection};

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
        // What the composer submits depends on the cursor, so it says so.
        self.draw_agent_composer(f, panes.composer, &selection);
    }

    /// Resolve what the rail cursor is on, clamping it to the rows that exist.
    fn agents_selection(&mut self) -> Selection {
        let lanes = self.lanes();
        let rows = self.rail_rows();
        let active = self.agent_index.min(rows.len().saturating_sub(1));
        self.agent_index = active;
        let node = self.selected_fleet_node();
        let lane_index = match rows.get(active) {
            Some(RailRow::Agent(row)) => row.lane_index().unwrap_or(0),
            _ => 0,
        };
        let task = match rows.get(active) {
            Some(RailRow::Agent(AgentRow::Sub { task, .. })) => Some(task.clone()),
            _ => None,
        };
        // A fleet row shows a declaration, so it is never "the conversation"
        // even though it leaves the lane cursor where it was.
        let on_orchestrator = node.is_none()
            && lanes
                .get(lane_index)
                .map(|l| l.role == AgentRole::Orchestrator)
                .unwrap_or(true);
        Selection {
            rows,
            active,
            lanes,
            lane_index,
            task,
            node,
            on_orchestrator,
        }
    }

    /// Divide `area` between the rail, the pane, and the composer.
    ///
    /// The rail is sized to its widest row rather than to a percentage: its
    /// labels are short and fixed-ish, and the transcript is what benefits from
    /// width. The threads strip takes rows only when more than one thread is
    /// open — with a single thread the pane title already names it.
    fn agents_panes(&mut self, area: Rect, selection: &Selection) -> AgentsPanes {
        let threads = self.thread_rows();
        let widest = selection
            .rows
            .iter()
            .map(|row| self.rail_row_line(row, &selection.lanes, false).width())
            .chain(threads.iter().map(|line| line.width()))
            .max()
            .unwrap_or(0);
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
        // The composer lives inside the right column, under the transcript it
        // belongs to, and grows with the draft.
        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(self.composer_height()),
            ])
            .split(columns[1]);
        // The work panel splits the transcript row, not the whole column: the
        // composer still spans the width it submits to, and a narrow terminal
        // keeps the transcript whole rather than showing two cramped halves.
        let show_work =
            self.selected_work(selection).is_some() && area.width >= work::MIN_WIDTH_FOR_WORK_PANE;
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
