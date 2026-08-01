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
use crate::ui::agents::{AgentRole, AgentRow};
use crate::worker::pty::SessionRow;

/// The label on the rail's "start a harness" row.
pub(in crate::ui::app) const NEW_HARNESS_LABEL: &str = "+ New harness";

/// One row of the Agents rail.
#[derive(Debug, Clone)]
pub enum RailRow {
    /// A lane, task sublane, or lane-list divider.
    ///
    /// The lane list's own `── functions ──` separator is an
    /// `AgentRow::Separator`; this variant is the group, not the row type.
    Agent(AgentRow),
    /// The action row that starts a harness of the operator's own.
    ///
    /// Sits directly under the orchestrator lane because that is where the eye
    /// already is — starting a terminal was otherwise a chord (`Ctrl-T`) with
    /// nothing on screen to suggest it exists, which is the same as not having
    /// it for anyone who has not read the bindings.
    NewHarness,
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
            RailRow::NewHarness => true,
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

    /// Whether this row is the "start a harness" action.
    pub fn is_new_harness(&self) -> bool {
        matches!(self, RailRow::NewHarness)
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
    /// cannot see. The `+ New harness` action sits between the two, directly
    /// under the orchestrator lane: it is what produces the group below it, and
    /// a device that hosts nothing cannot start one, so it is absent there
    /// rather than present and refusing.
    pub(super) fn rail_rows(&self) -> Vec<RailRow> {
        let lanes = self.lanes();
        let can_start = self.harnesses.is_some();
        let mut rows: Vec<RailRow> = Vec::new();
        let mut placed = !can_start;
        for row in self.agent_rows() {
            let orchestrator = matches!(&row, AgentRow::Lane { lane_index }
                if lanes.get(*lane_index).map(|l| l.role) == Some(AgentRole::Orchestrator));
            rows.push(RailRow::Agent(row));
            if orchestrator && !placed {
                rows.push(RailRow::NewHarness);
                placed = true;
            }
        }
        // No orchestrator lane yet — the very first frame, before any fold has
        // run. The action still belongs on screen, and the top is where the
        // orchestrator will appear above it.
        if !placed {
            rows.insert(0, RailRow::NewHarness);
        }
        let own = self.own_harness_rows();
        if !own.is_empty() {
            rows.push(RailRow::HarnessSeparator);
            rows.extend(own.into_iter().map(RailRow::Harness));
        }
        rows
    }

    /// How many local harnesses are waiting on the operator right now.
    ///
    /// Counts every live session on this device, not only the rows on the rail:
    /// a harness the orchestrator started and then got stuck on a permission
    /// prompt is exactly the case an operator needs told about, and it has no
    /// row of its own — it is somewhere inside a lane.
    ///
    /// The attached session is excluded. Its prompt is on screen in front of
    /// the person the count is for, so counting it would ask them to go and
    /// look at what they are already looking at.
    /// Reads the waiting *ids* rather than [`rows`](crate::worker::pty::PtyManager::rows),
    /// which clones every session's whole row. This runs on the render thread
    /// once per frame for the tab badge, so it takes one lock and copies the
    /// handful of ids that are actually waiting — usually none.
    pub(in crate::ui) fn harnesses_waiting(&self) -> usize {
        let Some(harnesses) = self.harnesses.as_ref() else {
            return 0;
        };
        Self::count_waiting(&harnesses.sessions.waiting_sessions(), &self.harness_focus)
    }

    /// The same count from an already-collected waiting set.
    ///
    /// The rail collects that set anyway to style its lanes, so the header count
    /// is derived from it instead of taking the lock a second time — and, more
    /// to the point, the header and the rows beneath it are then answering from
    /// one snapshot rather than from two taken a few microseconds apart.
    pub(in crate::ui) fn count_waiting(
        waiting: &std::collections::HashSet<String>,
        focus: &crate::ui::harness_pane::HarnessFocus,
    ) -> usize {
        waiting
            .iter()
            .filter(|id| !focus.is_attached_to(id))
            .count()
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
