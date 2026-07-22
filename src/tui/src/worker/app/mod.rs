//! The worker TUI's screen: state, keys, and render.
//!
//! Partitioned like the orchestrator's [`App`](crate::ui::app::App) — [`types`]
//! holds the data model, [`state`] construction and accessors, [`keys`] the
//! keyboard dispatcher, and [`render`] the ratatui draw.

pub mod keys;
pub mod render;
pub mod state;
pub mod types;

#[cfg(test)]
mod tests;

pub use state::WorkerWiring;
pub use types::{
    Confirm, ExecutionMode, Screen, SetupStep, WorkerApp, WorkerCmd, EXECUTION_MODES, TABS,
};
