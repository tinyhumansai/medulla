//! Pure view-model fold: turn the flat event stream into one lane per cognitive
//! tier plus one lane per connected roster agent / anonymous task / peer session,
//! with a row model for the Agents list and pre-wrapped transcript lines. A port
//! of the TS `deriveAgentLanes` / `agentRowModel` / `laneLines` essentials.
//!
//! Split by responsibility: [`types`] holds the lane/task/turn data model,
//! [`lanes`] the event fold that produces lanes, [`rows`] the Agents-list row
//! model, [`lines`] the transcript rendering, and [`keys`] the lane-key parser.
//! The small formatting helpers shared by the fold live in the private `fmt`
//! submodule. All public items are re-exported here so callers use
//! `medulla::ui::agents::*`.

mod claims;
mod fmt;
mod keys;
mod lanes;
mod lines;
mod review;
mod rows;
mod types;

#[cfg(test)]
mod tests;

pub use claims::{
    claimed_dirty_paths, evaluate_lane_claims, validate_claim_patterns, ClaimPatternError,
};
pub use keys::parse_task_key;
pub use lanes::derive_agent_lanes;
pub use lines::{lane_lines, task_lines};
pub use rows::{agent_row_model, ordered_tasks};
pub use types::{
    AgentLane, AgentRole, AgentRow, ClaimedPath, LaneClaim, LaneGuardBadge, LaneGuardReport, Line,
    TaskState, TaskStatus, TurnBlock,
};
