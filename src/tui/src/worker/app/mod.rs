//! The worker TUI's screen: state, keys, and render.
//!
//! Partitioned like the orchestrator's [`App`](crate::ui::app::App) — [`types`]
//! holds the data model, [`state`] construction and accessors, [`keys`] the
//! keyboard dispatcher, [`input`] pointer routing, and [`render`] the ratatui
//! draw.

pub mod input;
pub mod keys;
pub mod render;
pub mod state;
pub mod types;

// Unix-only: the screen's tests populate it with live `/bin/sh` sessions.
#[cfg(all(test, unix))]
mod tests;

pub use state::WorkerWiring;
pub use types::{
    Confirm, ExecutionMode, Screen, SetupStep, WorkerApp, WorkerCmd, EXECUTION_MODES, TABS,
};
