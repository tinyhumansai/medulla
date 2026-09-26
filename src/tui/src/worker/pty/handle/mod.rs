//! [`SessionHandle`] — one live PTY session's own state and its own locks.
//!
//! Everything the manager can do to a single session lives here, so the manager
//! itself is only a registry: find the handle, drop the registry lock, call a
//! method. See [`types`] for why the state is split the way it is.
//!
//! The behaviour is split by concern across sibling modules so no one file
//! carries the whole session lifecycle:
//!
//! - [`lifecycle`] — construction, killing, and reaping.
//! - [`state`] — read-mostly accessors and the operator-facing row projection.
//! - [`control`] — who may use a session next, and claiming a turn.
//! - [`io`] — writing bytes to the child and resizing the pty + emulator.
//! - [`screen`] — projecting the emulator's screen into a caller's buffer.

mod control;
mod io;
mod lifecycle;
mod osc52;
mod screen;
mod state;
mod types;

#[cfg(test)]
mod osc52_tests;
#[cfg(test)]
mod tests;

pub(crate) use types::{release_queued, SessionMeta};
pub use types::{SessionHandle, MAX_QUEUED_WRITE_BYTES};
