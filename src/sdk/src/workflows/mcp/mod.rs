//! Workflow-specific MCP definitions, restrictions, and operation dispatch.
//!
//! The shared protocol server lives in [`crate::mcp`]. This module owns the
//! `workflow_*` family so authored-workflow behavior stays beside the domain it
//! exposes, while the server can aggregate it with other tool families.

mod definitions;
mod dispatch;
mod evolve;
mod run_progress;

pub(crate) use definitions::definitions;
pub(crate) use dispatch::call;
#[cfg(test)]
pub(crate) use dispatch::scope_error_for;
pub use dispatch::TOOL_NAMES;
pub use evolve::{ToolMode, TOOL_MODE_ENV, TOOL_SCOPE_ENV};
