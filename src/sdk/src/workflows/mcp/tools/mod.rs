//! The tools a harness sees and the modules that define their behavior.
//!
//! Descriptions matter more here than anywhere else in this feature: they are
//! the whole of what a model knows before it acts. Definitions, dispatch, and
//! restricted evolution modes stay separate so each responsibility remains
//! readable.

mod definitions;
mod dispatch;
pub mod evolve;

pub use definitions::tool_definitions;
pub(super) use dispatch::call;
pub use dispatch::TOOL_NAMES;
pub use evolve::{ToolMode, TOOL_MODE_ENV};
