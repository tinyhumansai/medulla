//! The emulator surface the UI renders, and the cells it renders into.
//!
//! Thin delegation onto [`SessionHandle`](super::super::handle::SessionHandle):
//! the emulator lock belongs to the session, so a large snapshot on one no
//! longer excludes the reader thread of another. What each operation actually
//! does is documented on the handle.

use super::PtyManager;

impl PtyManager {
    /// Whether the child has turned bracketed-paste mode on (DECSET 2004).
    ///
    /// We are this child's terminal, so this is not a preference to guess at: a
    /// real terminal sends `ESC[200~` markers only to an application that asked
    /// for them, and sending them to one that did not delivers the escape bytes
    /// as literal keystrokes. It doubles as the readiness signal — a harness
    /// sets its terminal modes when its input layer comes up, so `true` means
    /// there is something listening to type at.
    ///
    /// `None` when the session is unknown.
    pub fn bracketed_paste(&self, id: &str) -> Option<bool> {
        Some(self.handle(id)?.bracketed_paste())
    }

    /// Which mouse protocol the child has turned on, if any.
    ///
    /// We are this child's terminal, so this is not a preference to guess at: a
    /// real terminal sends mouse reports only to an application that asked for
    /// them, and sending them to one that did not delivers the escape bytes as
    /// literal keystrokes — `ESC [ < 6 4 ; 4 ; 9 M` typed into a composer.
    ///
    /// The mode says *whether* to report, the encoding says *how*. Returns
    /// `None` when the session is unknown; `Some(MouseProtocolMode::None, _)`
    /// when the child wants no reports at all.
    pub fn mouse_protocol(
        &self,
        id: &str,
    ) -> Option<(vt100::MouseProtocolMode, vt100::MouseProtocolEncoding)> {
        Some(self.handle(id)?.mouse_protocol())
    }

    /// Move `id`'s emulator scrollback by `rows`, towards the history when `up`.
    ///
    /// This is *our* scrollback, not the child's: it walks the lines the
    /// emulator has retained as the child scrolled them off. It exists for
    /// harnesses that never turn mouse reporting on, where there is nothing to
    /// forward a wheel event to and the alternative is a wheel that does
    /// nothing at all.
    ///
    /// Returns the resulting offset, or `None` when the session is unknown.
    /// Bounded to one screenful — see the handle for why that ceiling is forced
    /// on us rather than chosen.
    pub fn scroll_history(&self, id: &str, rows: usize, up: bool) -> Option<usize> {
        Some(self.handle(id)?.scroll_history(rows, up))
    }

    /// Snap `id`'s emulator back to the live screen.
    ///
    /// A harness that repaints while the operator is reading history would
    /// otherwise keep painting behind them, unseen. Called when the pane is
    /// typed into: typing means "I am here now", not "keep showing me the past".
    pub fn scroll_to_live(&self, id: &str) {
        if let Some(session) = self.handle(id) {
            session.scroll_to_live();
        }
    }

    /// Render `id`'s current screen as `(rows_of_cells, cursor)`.
    ///
    /// Returns owned rows rather than a borrow of the emulator: the render pass
    /// must not hold the parser's lock while the reader thread wants it.
    pub fn screen_rows(&self, id: &str) -> Option<ScreenSnapshot> {
        Some(self.handle(id)?.snapshot())
    }

    /// Write `id`'s screen into `out` as plain text, one line per row.
    ///
    /// For deciding *what is on* the screen rather than drawing it. The caller
    /// owns the buffer so a poller can reuse one: this is read at 40 Hz per
    /// starting session while a prompt is being injected, and the path it
    /// replaces built a whole [`ScreenSnapshot`] and joined it, several times
    /// per tick.
    ///
    /// Returns whether the session exists; `out` is cleared either way.
    pub fn screen_text_into(&self, id: &str, out: &mut String) -> bool {
        match self.handle(id) {
            Some(session) => {
                session.text_into(out);
                true
            }
            None => {
                out.clear();
                false
            }
        }
    }

    /// Write `id`'s screen into `out` with whitespace removed and case folded.
    ///
    /// The form [`super::super::inject`] counts prompt needles in, produced in
    /// one pass off the emulator instead of a snapshot plus two more full-screen
    /// strings.
    ///
    /// Returns whether the session exists; `out` is cleared either way.
    pub fn screen_squashed_into(&self, id: &str, out: &mut String) -> bool {
        match self.handle(id) {
            Some(session) => {
                session.squashed_text_into(out);
                true
            }
            None => {
                out.clear();
                false
            }
        }
    }

    /// Resize a session's PTY and emulator to `cols` x `rows`.
    ///
    /// Both must move together: the child reflows to the PTY size, so an
    /// emulator of a different size would render a torn screen. Short-circuits
    /// when the size already matches, so calling it every frame is a lock and a
    /// compare rather than a `SIGWINCH` per redraw.
    pub fn resize(&self, id: &str, cols: u16, rows: u16) {
        if let Some(session) = self.handle(id) {
            session.resize(cols, rows);
        }
    }

    /// Write raw bytes to a session's PTY — the focused pane's keystrokes.
    ///
    /// Blocks only this session: a child that has stopped draining its input
    /// parks the write in the kernel, and that used to happen under the one lock
    /// every other session and the render loop needed.
    pub fn write(&self, id: &str, bytes: &[u8]) -> Result<(), String> {
        match self.handle(id) {
            Some(session) => session.write(bytes),
            None => Err(format!("no session {id}")),
        }
    }
}

mod types;
pub use types::ScreenCell;
pub use types::ScreenSnapshot;
