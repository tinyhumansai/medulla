//! The embedded harness terminal in the orchestrator's Agents tab.
//!
//! An agent lane used to show a transcript we reconstructed from the harness's
//! JSON stream. That reconstruction only ever existed because the harness was
//! started headless (`-p --output-format stream-json`), which suppresses its
//! interface. This module is the other choice: run `claude`/`codex` the way a
//! human runs them, on a pseudo-terminal, and show the interface they actually
//! paint.
//!
//! What that buys is not cosmetic. A transcript cannot show a harness stopped on
//! a permission dialog, a plan-mode prompt, or a model picker — those write no
//! records, so the lane read as "thinking" until the task timed out. A screen
//! shows them, and an *attached* screen lets the operator answer them.
//!
//! Responsibilities:
//! - [`LocalHarnesses`] — resolving "what is the cursor on" to a live session;
//! - [`HarnessFocus`] — which of the TUI and the harness owns the keyboard;
//! - [`keys`] — encoding a crossterm key back into the bytes a terminal sends.
//!
//! The emulator, the child processes, and the screen-to-ratatui translation are
//! not here: they are [`crate::worker::pty`] and [`crate::worker::screen`],
//! shared verbatim with the worker daemon's TUI so the two cannot drift.

use crate::worker::pty::{PtyManager, ScreenSnapshot};

pub mod keys;
pub mod mouse;
mod types;

#[cfg(test)]
mod tests;

pub use types::{HarnessFocus, LocalHarnesses};

/// How the focus chord is written in hints and titles.
///
/// The chord itself is `Ctrl-]`, chosen for the same reason telnet chose it: no
/// full-screen program binds it, so it can be reserved without taking anything
/// away from the harness. Every *other* key belongs to whoever currently has
/// focus — which is what makes an attached pane a real terminal rather than a
/// text box with opinions.
///
/// Recognising it is [`keys::is_focus_chord`], not a character comparison:
/// terminals do not deliver this key the way it is written.
pub const FOCUS_CHORD_LABEL: &str = "Ctrl-]";

impl LocalHarnesses {
    /// The live session serving `task_id`, if one is.
    ///
    /// `None` once the task settles: the runtime drops the record then, so a
    /// pane stops claiming a screen for work that is over rather than showing a
    /// dead one indefinitely.
    pub fn session_for_task(&self, task_id: &str) -> Option<String> {
        self.runtime.session_for_task(&self.hub_address, task_id)
    }

    /// The current screen of `session_id`, ready to render.
    ///
    /// Returns an owned snapshot: the render pass must not hold the emulator's
    /// lock while the reader thread wants it.
    pub fn screen(&self, session_id: &str) -> Option<ScreenSnapshot> {
        self.sessions.screen_rows(session_id)
    }

    /// Match the pane's geometry onto the child, so it reflows to what is
    /// actually visible.
    ///
    /// Called every frame from the render pass. `PtyManager::resize` short-
    /// circuits when the size already matches, so this is a lock and a compare
    /// in the steady state rather than a `SIGWINCH` per redraw.
    pub fn fit(&self, session_id: &str, cols: u16, rows: u16) {
        self.sessions.resize(session_id, cols, rows);
    }

    /// Send already-encoded bytes to a session's PTY.
    ///
    /// # Errors
    ///
    /// Fails when the session is unknown or has exited — an attached pane whose
    /// harness died should say so rather than swallow the operator's typing.
    pub fn write(&self, session_id: &str, bytes: &[u8]) -> Result<(), String> {
        self.sessions.write(session_id, bytes)
    }

    /// Scroll `session_id` by one wheel notch at pane-relative `(col, row)`.
    ///
    /// Two paths, chosen by what the child asked for rather than by what we
    /// would prefer:
    ///
    /// - the harness enabled mouse reporting (Claude Code and Codex both do), so
    ///   the notch is forwarded and *its* scrollback moves — which is the one
    ///   the operator means, because it holds the whole conversation rather than
    ///   the last screenful the emulator happened to retain;
    /// - it did not, so our emulator's own retained lines move instead. Not as
    ///   good, but a wheel that does something beats one that does nothing, and
    ///   forwarding to a child that never opted in would type escape bytes into
    ///   its composer.
    ///
    /// `rows` is how far to move our own history when that is the path taken;
    /// the forwarded path sends one notch and lets the harness decide what a
    /// notch means.
    pub fn scroll(&self, session_id: &str, col: u16, row: u16, up: bool, rows: usize) {
        if let Some((mode, encoding)) = self.sessions.mouse_protocol(session_id) {
            if let Some(bytes) = mouse::wheel(mode, encoding, col, row, up) {
                // A failed write means the child died; the pane notices on its
                // next frame, and a lost wheel notch is not worth a message.
                let _ = self.sessions.write(session_id, &bytes);
                return;
            }
        }
        self.sessions.scroll_history(session_id, rows, up);
    }

    /// Snap the pane back to the live screen.
    ///
    /// Only meaningful for a session being scrolled through our own emulator; a
    /// harness that owns its scrollback handles this itself.
    pub fn scroll_to_live(&self, session_id: &str) {
        self.sessions.scroll_to_live(session_id);
    }

    /// Whether `session_id` names a session that is still running.
    ///
    /// Attaching to an exited session would give the operator a keyboard wired
    /// to nothing, so the attach path checks this first.
    pub fn is_running(&self, session_id: &str) -> bool {
        self.sessions
            .row(session_id)
            .is_some_and(|row| row.state.is_running())
    }
}
