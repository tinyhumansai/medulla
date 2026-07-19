//! The pre-app login screen: a pure state machine the `main` pre-app loop drives
//! before the main TUI when the backend runtime needs a token.
//!
//! All async work (binding the loopback listener, opening the browser, awaiting
//! the callback, redeeming a one-time token, and `me()` verification) lives in
//! `main`. This module only owns state and rendering: [`LoginScreen::handle_key`]
//! turns keys into [`LoginCmd`]s, [`LoginScreen::apply`] folds [`LoginEvent`]s
//! from those async tasks back into state, and [`LoginScreen::draw`] renders the
//! centered panel. The loop reads [`LoginScreen::outcome`] to know when to stop.
//!
//! The module is split by responsibility: [`types`] holds the data model
//! (screen struct, `Cmd`/`Event`/`Outcome` enums, and the internal `Phase`),
//! [`state`] the key-handling/event-folding state machine, and [`draw`] the
//! ratatui rendering.

mod draw;
mod state;
mod types;

#[cfg(test)]
mod tests;

pub use types::{LoginCmd, LoginEvent, LoginOutcome, LoginScreen};
