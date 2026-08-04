//! The Workflows tab: a catalogue sidebar beside one content pane.
//!
//! The same two-pane shape as Routing and Settings — a narrow selector on the
//! left, one thing being looked at on the right. It began as four panes at once
//! (catalogue, canvas, inspector strip, copilot column) on the theory that an
//! operator's three questions are asked together. In practice that put four
//! bordered boxes and two independent transcripts on screen at all times, and
//! the graph — the thing actually being read — got whatever was left.
//!
//! So the content pane shows exactly one [`WorkflowView`] at a time and the
//! keys that switch views are the same ones that used to move focus between
//! panes: `c` for the copilot, `i` for the node's declaration, `Esc` back
//! towards the graph and then the list. Nothing that was reachable before is
//! unreachable now; it is one keystroke away instead of permanently on screen.
//!
//! The sidebar is sized to the catalogue it holds, via the same
//! [`crate::ui::multi_pane::sidebar_width`] the Agents rail uses.
//!
//! - [`rail`] — the catalogue and the selected workflow's runs.
//! - [`canvas`] — the laid-out graph. [`paint`] is the character grid under it.
//! - [`node_preview`] — a rich, persistent view of the selected step.
//! - [`inspector`] — the selected node's declaration, and its run detail.
//! - [`copilot`] — the conversation that edits the graph.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::Frame;

use super::super::types::{App, WorkflowFocus, WorkflowView};

mod canvas;
mod copilot;
mod inspector;
mod node_preview;
mod paint;
pub(in crate::ui::app) mod rail;

#[cfg(test)]
mod copilot_tests;

#[cfg(test)]
mod rail_tests;

#[cfg(test)]
mod tests;

/// Width of one node's slot, in columns: its marker, a space, and its name.
///
/// A node is a marker and a label rather than a drawn box. Boxes cost four rows
/// and twenty columns each to say what a coloured dot says in one row, and the
/// graphs worth looking at are the ones with too many nodes to fit — so the
/// space a box spends on its own border is exactly the space the graph needs.
pub(in crate::ui::app) const NODE_WIDTH: usize = 17;

/// Height of one node, in rows. A marker and its label are one line.
pub(in crate::ui::app) const NODE_HEIGHT: usize = 1;

/// Columns from one layer's left edge to the next: a node plus the gutter its
/// outgoing wires are routed through.
///
/// The gutter is nine columns rather than the four a wire needs, because a
/// branch's port name (`false`) is written along it — and a truncated port name
/// tells a reader nothing about which arm they are following.
pub(in crate::ui::app) const LAYER_STRIDE: usize = NODE_WIDTH + 9;

/// Rows from one lane's top edge to the next: a node plus a blank row, so the
/// wires between two stacked nodes have a row to run along.
pub(in crate::ui::app) const LANE_STRIDE: usize = NODE_HEIGHT + 1;

/// The most content columns the workflow rail may claim.
///
/// Mirrors the Agents rail: labels may influence the width up to this point,
/// but an unusually long workflow name or run detail is clipped instead of
/// shrinking the graph, inspector, or copilot beside it.
const RAIL_MAX_CONTENT: usize = 36;

impl App {
    /// Draw the Workflows tab.
    pub(super) fn draw_workflows_tab(&mut self, f: &mut Frame, area: Rect) {
        // Sized to the catalogue it holds rather than a fixed width, the same
        // way the Agents rail is sized — so a machine with short workflow names
        // does not give a third of the screen to whitespace, and one with long
        // names can still read them.
        let rail = self.workflow_sidebar_width(area.width);
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(rail), Constraint::Min(0)])
            .split(area);

        // Each pane is noted so a pointer drag reads one of them rather than
        // splicing two columns' text into every row. The canvas especially: its
        // rows are box-drawing characters, and a selection that also caught the
        // sidebar's prose is unreadable.
        self.note_pane(columns[0]);
        self.note_pane(columns[1]);

        self.draw_workflow_rail(f, columns[0]);
        match self.workflow_view() {
            WorkflowView::Graph => self.draw_workflow_canvas(f, columns[1]),
            WorkflowView::Inspector => self.draw_workflow_inspector(f, columns[1]),
            WorkflowView::Copilot => self.draw_workflow_copilot(f, columns[1]),
        }
    }

    /// Size the workflow rail from its content, bounded like the Agents rail.
    pub(in crate::ui::app) fn workflow_sidebar_width(&self, total: u16) -> u16 {
        let widest = self
            .workflow_rail_rows()
            .iter()
            .map(|row| self.workflow_rail_width(row))
            .max()
            .unwrap_or(0)
            .min(RAIL_MAX_CONTENT);
        crate::ui::multi_pane::sidebar_width(total, widest)
    }

    /// What the content pane is showing.
    ///
    /// Derived from focus and the inspector toggle rather than stored, so there
    /// is no third piece of state that can disagree with the two that already
    /// decide it. The copilot wins over the inspector because it is the one you
    /// step *into*; the graph is what everything else falls back to, being the
    /// thing the tab is for.
    pub(in crate::ui::app) fn workflow_view(&self) -> WorkflowView {
        if self.wf.focus == WorkflowFocus::Copilot {
            WorkflowView::Copilot
        } else if self.wf.inspector_open {
            WorkflowView::Inspector
        } else {
            WorkflowView::Graph
        }
    }
}
