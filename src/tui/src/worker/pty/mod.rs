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
//! - [`handle`] — [`SessionHandle`], one session's own state and its own locks.
//! - [`manager`] — [`PtyManager`], the registry of those handles.
//! - [`cell_text`] — the inline string one screen cell's text is stored in.
//! - [`sync`] — poison-tolerant lock helpers.
//! - [`types`] — the data model.
//!
//! The split between [`handle`] and [`manager`] is what makes this survive a
//! fan-out. Everything used to sit in one `Mutex<Vec<PtySession>>`, so a reader
//! thread stamping a timestamp, a render pass reading a screen, and a write into
//! a wedged child all contended on the same process-wide lock. Now the registry
//! is read-mostly and every session owns its own state; see the two modules'
//! docs for the details.

pub mod cell_text;
pub mod dialog;
pub mod handle;
pub mod inject;
pub mod launch;
pub mod manager;
mod sync;
pub mod types;

// Unix-only: every test here drives a real child on a real pseudo-terminal
// via `/bin/sh`, which Windows has no equivalent of. The pty layer itself is
// portable; its tests are not.
#[cfg(all(test, unix))]
mod tests;

pub use cell_text::CellText;
pub use handle::SessionHandle;
pub use inject::inject_prompt;
pub use manager::{PtyManager, ScreenCell, ScreenSnapshot};
pub use types::{LaunchSpec, PtyState, SessionRow};
