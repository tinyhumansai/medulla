//! The Agents rail: one cursor over the lanes *and* the declared fleet.
//!
//! The two lists answer adjacent questions — what is running, and what it is
//! running on — and an operator moves between them constantly: an agent stalls,
//! and the next thing you want is the harness it sits on and how much budget
//! that harness has left. Splitting them across tabs meant losing your place in
//! one to look at the other, so they share a rail and a selection here.
//!
//! Rows keep their own models ([`AgentRow`] from the event fold, [`FleetNode`]
//! from the declared capacity); this module only concatenates them, tracks which
//! is selected, and answers what the detail pane should show.

use crate::ui::agents::AgentRow;
use crate::ui::fleet::FleetNode;

use super::types::App;

/// One row of the Agents rail.
#[derive(Debug, Clone)]
pub enum RailRow {
    /// A lane, task sublane, or lane-list divider.
    Agent(AgentRow),
    /// A non-selectable heading between the two lists.
    Divider(&'static str),
    /// A host, harness, workspace, agent, or template from the declared fleet.
    Fleet(FleetNode),
}

impl RailRow {
    /// Whether the cursor may land on this row.
    pub fn selectable(&self) -> bool {
        match self {
            RailRow::Agent(row) => row.selectable(),
            RailRow::Divider(_) => false,
            RailRow::Fleet(node) => node.kind.selectable(),
        }
    }
}

impl App {
    /// The rail's rows: the lanes, then the declared fleet under a divider.
    ///
    /// The fleet half is omitted entirely when nothing is declared, so a session
    /// with no capacity to show is byte-identical to the lane list alone.
    pub(super) fn rail_rows(&self) -> Vec<RailRow> {
        let mut rows: Vec<RailRow> = self.agent_rows().into_iter().map(RailRow::Agent).collect();
        let fleet = self.fleet_rows();
        if !fleet.is_empty() {
            rows.push(RailRow::Divider("── fleet ──"));
            rows.extend(fleet.into_iter().map(RailRow::Fleet));
        }
        rows
    }

    /// The selection key of the fleet row under the cursor. Test/inspection seam.
    pub fn selected_fleet_node_key(&self) -> Option<String> {
        self.selected_fleet_node().map(|node| node.key)
    }

    /// The fleet node under the cursor, when the cursor is in the fleet half.
    ///
    /// `None` for a lane row, which is what tells the right-hand pane to render
    /// a transcript rather than a declaration.
    pub(super) fn selected_fleet_node(&self) -> Option<FleetNode> {
        match self.rail_rows().get(self.agent_index) {
            Some(RailRow::Fleet(node)) if node.kind.selectable() => Some(node.clone()),
            _ => None,
        }
    }
}
