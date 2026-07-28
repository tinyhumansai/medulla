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

use super::types::App;
use crate::ui::agents::AgentRow;

/// One row of the Agents rail.
#[derive(Debug, Clone)]
pub enum RailRow {
    /// A lane, task sublane, or lane-list divider.
    ///
    /// The only variant, now that the declared fleet no longer hangs below the
    /// lanes. Kept as an enum rather than collapsed into `AgentRow` so the rail
    /// kept its seam: the lane list's own `── functions ──` separator is an
    /// `AgentRow::Separator`, and a future second group would land here.
    Agent(AgentRow),
}

impl RailRow {
    /// Whether the cursor may land on this row.
    pub fn selectable(&self) -> bool {
        match self {
            RailRow::Agent(row) => row.selectable(),
        }
    }
}

impl App {
    /// The rail's rows: the agent lanes.
    ///
    /// The declared fleet used to hang underneath these, and it was a third
    /// rendering of things that already had two homes. Its agents were the very
    /// lanes above the divider, so a worker that was both connected and declared
    /// appeared twice; its hosts and harnesses are the Routing tab's Harnesses
    /// page, which reads the same `fleet_capacity()`; and its templates were
    /// already excluded here in favour of Routing's Agent Templates page. What
    /// remained was duplication, so the rail now shows what is *running* and
    /// nothing else.
    pub(super) fn rail_rows(&self) -> Vec<RailRow> {
        self.agent_rows().into_iter().map(RailRow::Agent).collect()
    }
}
