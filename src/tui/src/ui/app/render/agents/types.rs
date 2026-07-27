//! Data model for the Agents tab's render pass: what the rail cursor is on, and
//! the panes that fall out of it.

use ratatui::layout::Rect;

use crate::ui::agents::{AgentLane, TaskState};
use crate::ui::fleet::FleetNode;

use super::super::super::rail::RailRow;

/// What the rail cursor is pointing at, resolved once per draw.
///
/// The rail and the transcript both need this and must agree: the pane titled
/// `harness · claude-code` has to be the row the cursor is on, and computing it
/// twice is how the two drift apart on an edge case. So it is derived once, at
/// the top of the draw, and handed to both.
pub(super) struct Selection {
    /// Every rail row, in display order.
    pub(super) rows: Vec<RailRow>,
    /// The clamped cursor index into `rows`.
    pub(super) active: usize,
    /// Every lane the fold produced, in display order.
    pub(super) lanes: Vec<AgentLane>,
    /// The lane whose transcript the pane shows, when the cursor is on one.
    pub(super) lane_index: usize,
    /// The task sublane under the cursor, if the cursor is on one.
    pub(super) task: Option<TaskState>,
    /// The fleet node under the cursor, if the cursor is in the fleet half.
    /// `Some` is what switches the pane from a transcript to a declaration.
    pub(super) node: Option<FleetNode>,
    /// Whether the pane is showing the operator's own conversation, which
    /// scrolls separately and renders the chat log rather than model calls.
    pub(super) on_orchestrator: bool,
}

impl Selection {
    /// The lane the pane describes, if any.
    pub(super) fn lane(&self) -> Option<&AgentLane> {
        self.lanes.get(self.lane_index)
    }
}

/// Where each pane of the Agents tab landed this draw.
pub(super) struct AgentsPanes {
    /// The threads strip above the rail; zero-height while only one is open.
    pub(super) threads: Rect,
    /// The rail: lanes, the declared fleet, and the template catalog.
    pub(super) rail: Rect,
    /// The transcript or declaration pane.
    pub(super) pane: Rect,
    /// The work panel beside the transcript; `None` when the selection has no
    /// work to show or the terminal is too narrow to spare the columns.
    pub(super) work: Option<Rect>,
    /// The composer, under the pane it submits to.
    pub(super) composer: Rect,
}
