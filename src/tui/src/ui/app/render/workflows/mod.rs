//! The Workflows tab: the catalogue, the graph, and the copilot.
//!
//! Three panes across, because the three questions an operator has about a
//! workflow are asked together: *which* plan (the rail), *what it does* (the
//! canvas, with the last run overlaid on it), and *change it* (the copilot).
//! Splitting them across subpages meant reading a graph on one screen and
//! editing it on another, with no way to see the edit land.
//!
//! The canvas keeps the middle and the largest share, because it is the thing
//! being read. The rail is sized to the catalogue it holds — the same
//! [`crate::ui::multi_pane::sidebar_width`] the Agents rail uses — and the
//! copilot is fixed-width, because it holds short lines and a percentage split
//! would give it half a wide terminal for no benefit.
//!
//! Three columns only fit on a wide terminal. Below
//! `MIN_WIDTH_FOR_COPILOT` the copilot and the canvas share the content pane and
//! focus decides which is showing, rather than all three being squeezed until
//! none of them is readable.
//!
//! - [`rail`] — the catalogue and the selected workflow's runs.
//! - [`canvas`] — the laid-out graph. [`paint`] is the character grid under it.
//! - [`inspector`] — the selected node's declaration, and its run detail.
//! - [`copilot`] — the conversation that edits the graph.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;

use super::super::types::{App, WorkflowFocus};

mod canvas;
mod copilot;
mod inspector;
mod paint;
mod rail;

#[cfg(test)]
mod tests;

/// Width of the copilot pane, in columns.
pub(in crate::ui::app) const COPILOT_WIDTH: u16 = 38;

/// The narrowest terminal that still shows the copilot beside the graph.
///
/// Below this the three columns do not fit: a 30-column rail and a 38-column
/// copilot leave a standard 80-column terminal twelve columns of canvas, which
/// is less than one node box. So on a narrow screen the copilot takes the
/// content pane when it is focused and yields it to the graph when it is not —
/// the same responsive move the Agents tab makes with its work pane.
const MIN_WIDTH_FOR_COPILOT: u16 = 110;

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
    /// Columns the copilot claims beside the canvas, or zero when the terminal
    /// is too narrow to seat all three panes.
    ///
    /// The single place the responsive decision is made, so the layout and the
    /// canvas's own width arithmetic ([`App::visible_layers`]) cannot disagree
    /// about how much room the graph has.
    pub(in crate::ui::app) fn copilot_column_width(&self) -> u16 {
        if self.area.width >= MIN_WIDTH_FOR_COPILOT {
            COPILOT_WIDTH
        } else {
            0
        }
    }

    /// Draw the Workflows tab.
    pub(super) fn draw_workflows_tab(&mut self, f: &mut Frame, area: Rect) {
        // Sized to the catalogue it holds rather than a fixed 30 columns, the
        // same way the Agents rail is sized — so a machine with short workflow
        // names does not give a third of the screen to whitespace, and one with
        // long names can still read them.
        let widest = self
            .workflow_rail_rows()
            .iter()
            .map(|row| self.workflow_rail_width(row))
            .max()
            .unwrap_or(0);
        let rail = crate::ui::multi_pane::sidebar_width(area.width, widest);

        let copilot_column = self.copilot_column_width();
        let beside = copilot_column > 0;
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(rail),
                Constraint::Min(0),
                Constraint::Length(copilot_column),
            ])
            .split(area);

        // On a narrow terminal the copilot and the graph share the content pane,
        // and focus decides which of them is showing. The graph is the default
        // because it is the thing being read; the copilot is what you step into.
        let copilot_only = !beside && self.wf.focus == WorkflowFocus::Copilot;
        if copilot_only {
            self.note_pane(columns[0]);
            self.note_pane(columns[1]);
            self.draw_workflow_rail(f, columns[0]);
            self.draw_workflow_copilot(f, columns[1]);
            return;
        }

        let inspector = if self.wf.inspector_open {
            INSPECTOR_HEIGHT
        } else {
            INSPECTOR_STRIP
        };
        let middle = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(inspector)])
            .split(columns[1]);

        // Each pane is noted so a pointer drag reads one of them rather than
        // splicing three columns' text into every row. The canvas especially:
        // its rows are box-drawing characters interleaved with two neighbours'
        // prose, and a selection across all three is unreadable.
        for pane in [columns[0], middle[0], middle[1]] {
            self.note_pane(pane);
        }

        self.draw_workflow_rail(f, columns[0]);
        self.draw_workflow_canvas(f, middle[0]);
        self.draw_workflow_inspector(f, middle[1]);
        if beside {
            self.note_pane(columns[2]);
            self.draw_workflow_copilot(f, columns[2]);
        }
    }
}
