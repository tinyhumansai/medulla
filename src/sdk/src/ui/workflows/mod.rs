//! The UI-facing view of installed workflows: their listings, their graphs, and
//! their runs.
//!
//! Neutral data rather than ratatui widgets, matching the rest of [`crate::ui`]:
//! the SDK decides *what* a surface shows and the app crate decides how it is
//! drawn, so the same judgement can reach a terminal, a log, or a test without
//! three renderings of it.
//!
//! - [`rows`] — the catalogue and run-history listings.
//! - [`graph`] — laying a graph out on a character grid, and moving a cursor
//!   through it.
//! - [`inspect`] — what a selected node says about itself, and how a run marks
//!   the graph up.
//! - [`journal`] — what a workflow has learned, and what it suggests changing.
//! - [`copilot`] — the transcript model for the graph-editing assistant.
//! - [`progress`] — reading that assistant's progress frames as tool calls or
//!   chatter.
//! - [`run_view`] — what one run says about itself: its inputs, its origin, and
//!   how far it got.

pub mod copilot;
pub mod graph;
pub mod inspect;
pub mod journal;
pub mod progress;
pub mod rows;
pub mod run_view;

pub use copilot::{CopilotState, CopilotTurn, TurnRole};
pub use graph::{GraphLayout, Move, PlacedEdge, PlacedNode};
pub use inspect::{
    find_node as find_node_in, node_detail, DetailRow, NodeRun, NodeRunState, RunOverlay,
};
pub use journal::{actionable, displayed, note_rows, pending, proposal_detail, proposal_rows};
pub use progress::{classify as classify_progress, Progress};
pub use rows::{run_rows, status_color, status_label, workflow_rows, WorkflowRow};
pub use run_view::{human_duration, run_overview, short_session, value_text};
