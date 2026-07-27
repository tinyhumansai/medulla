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
//! - [`copilot`] — the transcript model for the graph-editing assistant.

pub mod copilot;
pub mod graph;
pub mod inspect;
pub mod rows;

pub use copilot::{CopilotState, CopilotTurn, TurnRole};
pub use graph::{GraphLayout, Move, PlacedEdge, PlacedNode};
pub use inspect::{node_detail, DetailRow, NodeRun, NodeRunState, RunOverlay};
pub use rows::{run_rows, status_color, status_label, workflow_rows, WorkflowRow};
