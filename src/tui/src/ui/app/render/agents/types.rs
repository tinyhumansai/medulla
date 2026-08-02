//! Data model for the Agents tab's render pass: what the rail cursor is on, and
//! the panes that fall out of it.

use ratatui::layout::Rect;

use crate::ui::agents::{AgentLane, TaskState};

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
    /// Whether the pane is showing the operator's own conversation, which
    /// scrolls separately and renders the chat log rather than model calls.
    pub(super) on_orchestrator: bool,
    /// The live local harness session this row resolves to, when it resolves to
    /// one.
    ///
    /// Decided here rather than at draw time because it changes the *layout*,
    /// not just the contents: a harness paints its own composer, so ours has no
    /// rows and the work panel no columns. Resolving it after the split would
    /// mean laying out for a transcript and then drawing a terminal into it.
    pub(super) harness: Option<String>,
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
    /// The rail: the agent lanes and their tasks.
    pub(super) rail: Rect,
    /// The transcript, or the watched worker screen when one is streaming.
    pub(super) pane: Rect,
    /// The work panel beside the transcript; `None` when the selection has no
    /// work to show or the terminal is too narrow to spare the columns.
    pub(super) work: Option<Rect>,
    /// The composer, under the pane it submits to.
    pub(super) composer: Rect,
}
