//! The Workflows tab: the catalogue, the graph, and the copilot.
//!
//! Three panes across, because the three questions an operator has about a
//! workflow are asked together: *which* plan (the rail), *what it does* (the
//! canvas, with the last run overlaid on it), and *change it* (the copilot).
//! Splitting them across subpages meant reading a graph on one screen and
//! editing it on another, with no way to see the edit land.
//!
//! The canvas keeps the middle and the largest share, because it is the thing
//! being read. The rail and the copilot are fixed-width: both hold short lines,
//! and a percentage split gives them half a wide terminal for no benefit.
//!
//! - [`rail`] — the catalogue and the selected workflow's runs.
//! - [`canvas`] — the laid-out graph. [`paint`] is the character grid under it.
//! - [`inspector`] — the selected node's declaration, and its run detail.
//! - [`copilot`] — the conversation that edits the graph.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;

use super::super::types::App;

mod canvas;
mod copilot;
mod inspector;
mod paint;
mod rail;

#[cfg(test)]
mod tests;

/// Width of the catalogue rail, in columns.
pub(in crate::ui::app) const RAIL_WIDTH: u16 = 30;

/// Width of the copilot pane, in columns.
pub(in crate::ui::app) const COPILOT_WIDTH: u16 = 38;

/// Width of one node's box, in columns.
pub(in crate::ui::app) const NODE_WIDTH: usize = 18;

/// Height of one node's box, in rows.
pub(in crate::ui::app) const NODE_HEIGHT: usize = 4;

/// Columns from one layer's left edge to the next: a node box plus the gutter
/// its outgoing wires are routed through.
///
/// The gutter is ten columns rather than the four a wire needs, because a
/// branch's port name (`false`) is written along it — and a truncated port name
/// tells a reader nothing about which arm they are following.
pub(in crate::ui::app) const LAYER_STRIDE: usize = NODE_WIDTH + 10;

/// Rows from one lane's top edge to the next: a node box plus a blank row, so
/// two stacked boxes do not share a border line.
pub(in crate::ui::app) const LANE_STRIDE: usize = NODE_HEIGHT + 1;

/// Rows the inspector takes when it is open.
const INSPECTOR_HEIGHT: u16 = 10;

/// Rows the inspector takes when it is closed — a one-line summary of the
/// selected node, which is enough to know what the cursor is on.
const INSPECTOR_STRIP: u16 = 3;

impl App {
    /// Draw the Workflows tab.
    pub(super) fn draw_workflows_tab(&mut self, f: &mut Frame, area: Rect) {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(RAIL_WIDTH),
                Constraint::Min(0),
                Constraint::Length(COPILOT_WIDTH),
            ])
            .split(area);

        let inspector = if self.wf.inspector_open {
            INSPECTOR_HEIGHT
        } else {
            INSPECTOR_STRIP
        };
        let middle = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(inspector)])
            .split(columns[1]);

        self.draw_workflow_rail(f, columns[0]);
        self.draw_workflow_canvas(f, middle[0]);
        self.draw_workflow_inspector(f, middle[1]);
        self.draw_workflow_copilot(f, columns[2]);
    }
}
