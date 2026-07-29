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
use crate::worker::pty::SessionRow;

/// One row of the Agents rail.
#[derive(Debug, Clone)]
pub enum RailRow {
    /// A lane, task sublane, or lane-list divider.
    ///
    /// The lane list's own `── functions ──` separator is an
    /// `AgentRow::Separator`; this variant is the group, not the row type.
    Agent(AgentRow),
    /// The `── your harnesses ──` divider above the operator's own sessions.
    HarnessSeparator,
    /// A harness the operator started, which no lane will ever describe.
    ///
    /// Lanes are folded from task events, so a session nothing dispatched into
    /// produces none — which is exactly the state an unmanaged harness lives in.
    /// Without its own group it would be running, costing tokens, and invisible.
    Harness(SessionRow),
}

impl RailRow {
    /// Whether the cursor may land on this row.
    pub fn selectable(&self) -> bool {
        match self {
            RailRow::Agent(row) => row.selectable(),
            RailRow::HarnessSeparator => false,
            RailRow::Harness(_) => true,
        }
    }

    /// The PTY session this row names, when it names one directly.
    pub fn session_id(&self) -> Option<&str> {
        match self {
            RailRow::Harness(row) => Some(row.id.as_str()),
            _ => None,
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
    /// Operator-started harnesses hang below the lanes under their own divider,
    /// because they are the one thing running on this device that the event fold
    /// cannot see.
    pub(super) fn rail_rows(&self) -> Vec<RailRow> {
        let mut rows: Vec<RailRow> = self.agent_rows().into_iter().map(RailRow::Agent).collect();
        let own = self.own_harness_rows();
        if !own.is_empty() {
            rows.push(RailRow::HarnessSeparator);
            rows.extend(own.into_iter().map(RailRow::Harness));
        }
        rows
    }

    /// The harnesses this operator started, oldest first.
    ///
    /// Exited ones stay listed: the last screen is often the reason it exited,
    /// and a row that vanishes on failure is a row that hides the failure. They
    /// leave when the operator forgets them.
    pub(super) fn own_harness_rows(&self) -> Vec<SessionRow> {
        let Some(harnesses) = self.harnesses.as_ref() else {
            return Vec::new();
        };
        let mut rows: Vec<SessionRow> = harnesses
            .sessions
            .rows()
            .into_iter()
            .filter(|row| row.user_spawned)
            .collect();
        rows.sort_by_key(|row| row.started_at);
        rows
    }
}
