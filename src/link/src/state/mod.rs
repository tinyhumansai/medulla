//! Synchronised state objects: the trait, and the two channels of protocol §4.4.
//!
//! Channel 0 ([`MessageQueue`]) is reliable and ordered — nothing is dropped, so
//! a peer that was away receives everything it missed. Channel 1 ([`RowGrid`])
//! is latest-wins — a peer that was away receives the current grid in one diff.
//! Both are the same mechanism; only the diff differs, which is the reason SSP
//! fits medulla rather than merely working for it.

mod grid;
mod queue;
mod types;

#[cfg(test)]
mod tests;

pub use grid::{RowGrid, MAX_GRID_ROWS};
pub use queue::{MessageQueue, QueueLimits};
pub use types::{SspState, StateError};

/// Channel 0: reliable, ordered task frames.
pub const CHANNEL_MESSAGES: u8 = 0;

/// Channel 1: the latest-wins screen grid.
pub const CHANNEL_SCREEN: u8 = 1;
