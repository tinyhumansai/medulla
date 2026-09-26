//! Stable identity and movement for the Sessions rail cursor.
//!
//! The rail is rebuilt every frame, so its cursor records the selected row's
//! durable identity and resolves that identity against the current rows.

use super::{RailAnchor, RailRow};
use crate::ui::agents::AgentLane;
use crate::ui::app::types::{App, RailHit};

/// The identity of a selectable `row`, if it has one.
pub(in crate::ui::app) fn rail_anchor(row: &RailRow, lanes: &[AgentLane]) -> Option<RailAnchor> {
    match row {
        // A dispatched row remains identified by its task after the local PTY
        // is discovered. PTY enrichment must not move the cursor off that task
        // while the live rail is being rebuilt asynchronously.
        RailRow::Session(session) => session
            .task
            .as_ref()
            .and_then(|task| {
                session.lane_index.and_then(|index| {
                    lanes.get(index).map(|lane| RailAnchor::Task {
                        lane: lane.key.clone(),
                        task_id: task.task_id.clone(),
                    })
                })
            })
            .or_else(|| {
                session
                    .local
                    .as_ref()
                    .map(|local| RailAnchor::Session(local.id.clone()))
            }),
        RailRow::NewSession => Some(RailAnchor::NewSession),
        RailRow::WorkflowRun(row) => Some(RailAnchor::WorkflowRun(row.run.run_id.clone())),
        RailRow::Overflow { lane_index, .. } => lanes
            .get(*lane_index)
            .map(|lane| RailAnchor::Overflow(lane.key.clone())),
        RailRow::Host(_) | RailRow::Group(_) => None,
    }
}

/// Resolves an anchored cursor to its present offset, using `fallback` when gone.
pub(in crate::ui::app) fn resolve_rail_cursor(
    rows: &[RailRow],
    lanes: &[AgentLane],
    anchor: Option<&RailAnchor>,
    fallback: usize,
) -> usize {
    if rows.is_empty() {
        return 0;
    }
    anchor
        .and_then(|anchor| {
            rows.iter()
                .position(|row| rail_anchor(row, lanes).as_ref() == Some(anchor))
        })
        // Retaining a task can consume the final hidden row. In that case the
        // overflow anchor cannot be retained, but resolving by its old offset
        // would select the newly revealed task. Relocate to the lane's first
        // remaining row instead, which is stable as sessions appear beneath it.
        .or_else(|| match anchor {
            Some(RailAnchor::Overflow(lane)) => rows.iter().position(|row| {
                row.lane_index()
                    .and_then(|index| lanes.get(index))
                    .is_some_and(|candidate| candidate.key == *lane)
            }),
            _ => None,
        })
        .unwrap_or_else(|| fallback.min(rows.len() - 1))
}

impl App {
    /// Resolves the rail cursor against rows and lanes already collected by a caller.
    pub(in crate::ui::app) fn rail_cursor_in(
        &self,
        rows: &[RailRow],
        lanes: &[AgentLane],
    ) -> usize {
        resolve_rail_cursor(rows, lanes, self.agent_anchor.as_ref(), self.rail_index)
    }

    /// Resolves the stored cursor against one fresh rail and lane snapshot.
    pub(in crate::ui::app) fn rail_cursor(&self) -> usize {
        let lanes = self.lanes();
        let rows = self.rail_rows_in(&lanes);
        self.rail_cursor_in(&rows, &lanes)
    }

    /// Moves the cursor to `index` and remembers the selected row by identity.
    ///
    /// Test-only: every production caller already holds the rows and lanes it
    /// resolved the index against, and re-deriving them here would risk moving
    /// the cursor against a rail one frame newer than the one it was aimed at.
    #[cfg(test)]
    pub(in crate::ui::app) fn set_rail_cursor(&mut self, index: usize) {
        let lanes = self.lanes();
        let rows = self.rail_rows_in(&lanes);
        self.set_rail_cursor_in(&rows, &lanes, index);
    }

    /// Moves the cursor using rows and lanes already collected by a caller.
    pub(in crate::ui::app) fn set_rail_cursor_in(
        &mut self,
        rows: &[RailRow],
        lanes: &[AgentLane],
        index: usize,
    ) {
        self.rail_index = index.min(rows.len().saturating_sub(1));
        self.agent_anchor = rows
            .get(self.rail_index)
            .and_then(|row| rail_anchor(row, lanes));
    }

    /// Restores the cursor state captured with a rendered pointer target.
    pub(in crate::ui::app) fn set_rendered_rail_cursor(&mut self, hit: &RailHit) {
        self.rail_index = hit.index;
        self.agent_anchor = hit.anchor.clone();
    }

    /// Returns the rail cursor to its initial position without retaining its anchor.
    pub(in crate::ui::app) fn reset_rail_cursor(&mut self) {
        self.rail_index = 0;
        self.agent_anchor = None;
    }
}
