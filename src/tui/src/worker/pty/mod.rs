//! PTY-backed harness sessions: real `claude`/`codex`/`opencode` processes run
//! in their **interactive** mode on pseudo-terminals, with their screens parsed
//! by a terminal emulator so they can be embedded in the worker TUI.
//!
//! This is the counterpart to the SDK's headless session layer, not a
//! replacement for it. The headless path exists to *extract* structured events
//! from a harness; this one exists to *show* the harness as it actually looks.
//! They differ at the first argument: `-p --output-format stream-json` suppresses
//! the interface, and here we want it.
//!
//! Living in the app crate is deliberate and matches
//! [`crate::harness_pty`]: this is process and terminal wiring, so the SDK stays
//! free of `portable-pty`, `vt100`, and `crossterm`.
//!
//! - [`launch`] — the interactive argv and the input-injection encoding.
//! - [`inject`] — the timing and mode choreography that gets a prompt accepted.
//! - [`dialog`] — recognising a harness blocked on a startup dialog.
//! - [`manager`] — [`PtyManager`], which owns the children, the emulators, and
//!   the reader threads.
//! - [`types`] — the data model.

pub mod dialog;
pub mod inject;
pub mod launch;
pub mod manager;
pub mod types;

#[cfg(test)]
#[path = "dialog_tests.rs"]
mod dialog_tests;
// Unix-only: every test here drives a real child on a real pseudo-terminal
// via `/bin/sh`, which Windows has no equivalent of. The pty layer itself is
// portable; its tests are not.
#[cfg(all(test, unix))]
mod tests;

pub use inject::inject_prompt;
pub use manager::{PtyManager, ScreenCell, ScreenSnapshot};
pub use types::{LaunchSpec, PtyState, SessionRow};
