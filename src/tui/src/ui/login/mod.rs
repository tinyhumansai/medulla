//! The pre-app login screen: a pure state machine the `main` pre-app loop drives
//! before the main TUI when the backend runtime needs a token.
//!
//! Two sign-in methods are offered because one of them cannot work everywhere.
//! [`SignInMethod::Browser`] is the RFC 8252 loopback flow, which needs a
//! browser on this machine; over SSH its callback would land on the browser
//! host's `127.0.0.1` and never reach the listener. [`SignInMethod::Code`] is
//! the flow for that case: it shows a URL to open on any device and takes back
//! the one-time code that page produces. The operator chooses the method first,
//! then the provider, so the choice that decides whether sign-in can succeed at
//! all is not buried behind a provider.
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

pub use types::{LoginCmd, LoginEvent, LoginOutcome, LoginScreen, SignInMethod};
