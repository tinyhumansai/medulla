//! The worker-daemon TUI — `medulla daemon --tui`.
//!
//! A separate surface from the orchestrator TUI, and deliberately so: the
//! orchestrator's Workers tab is about remote peers this machine delegates
//! *to*, whereas this is the machine *doing* the work. What it shows is the
//! daemon's own state — the harness sessions running on it, the peers allowed to
//! reach it, and the requests waiting on a decision.
//!
//! - [`pty`] — live harness processes on pseudo-terminals, with their screens
//!   parsed for embedding.
//! - [`screen`] — the emulator-grid → ratatui translation.
//! - [`app`] — the screen itself: sessions, contacts, and pending requests.
//! - [`executor`] — runs a peer's delegated task inside a live session.

pub mod app;
pub mod executor;
// Unix-only: these run a fake harness as a `/bin/sh` script on a pty.
#[cfg(all(test, unix))]
mod executor_tests;
pub mod pty;
pub mod screen;
pub mod trust;
#[cfg(test)]
#[path = "trust_tests.rs"]
mod trust_tests;

pub use app::{WorkerApp, WorkerWiring};
