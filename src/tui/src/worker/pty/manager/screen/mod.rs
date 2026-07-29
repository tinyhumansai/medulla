//! The emulator surface the UI renders, and the cells it renders into.

use portable_pty::PtySize;

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
        let sessions = self.inner.sessions.lock().unwrap();
        let session = sessions.iter().find(|s| s.row.id == id)?;
        let parser = session.screen.lock().unwrap();
        Some(parser.screen().bracketed_paste())
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
        let sessions = self.inner.sessions.lock().unwrap();
        let session = sessions.iter().find(|s| s.row.id == id)?;
        let parser = session.screen.lock().unwrap();
        let screen = parser.screen();
        Some((
            screen.mouse_protocol_mode(),
            screen.mouse_protocol_encoding(),
        ))
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
    ///
    /// **Bounded to one screenful**, and not by choice. `vt100` 0.15's
    /// `set_scrollback` clamps the offset to the number of retained lines, but
    /// its `visible_rows` then computes `visible_row_count - offset` — so any
    /// offset larger than the screen height underflows and panics inside the
    /// crate, taking the TUI with it. Clamping here is the fix we can make from
    /// outside it. The practical effect is that this fallback reaches one screen
    /// back, not the full retained history.
    ///
    /// That ceiling is tolerable because this is the *fallback*: Claude Code and
    /// Codex both enable mouse reporting, so their wheel events are forwarded and
    /// they scroll their own (complete) history. This path only serves a harness
    /// that takes no mouse input at all.
    pub fn scroll_history(&self, id: &str, rows: usize, up: bool) -> Option<usize> {
        let sessions = self.inner.sessions.lock().unwrap();
        let session = sessions.iter().find(|s| s.row.id == id)?;
        let mut parser = session.screen.lock().unwrap();
        let (visible_rows, _) = parser.screen().size();
        let current = parser.screen().scrollback();
        let wanted = if up {
            current.saturating_add(rows)
        } else {
            current.saturating_sub(rows)
        };
        parser.set_scrollback(wanted.min(usize::from(visible_rows)));
        // Read back rather than returning `wanted`: the emulator applies its own
        // clamp on top of ours, so this is the only honest answer.
        Some(parser.screen().scrollback())
    }

    /// Snap `id`'s emulator back to the live screen.
    ///
    /// A harness that repaints while the operator is reading history would
    /// otherwise keep painting behind them, unseen. Called when the pane is
    /// typed into: typing means "I am here now", not "keep showing me the past".
    pub fn scroll_to_live(&self, id: &str) {
        let sessions = self.inner.sessions.lock().unwrap();
        let Some(session) = sessions.iter().find(|s| s.row.id == id) else {
            return;
        };
        session.screen.lock().unwrap().set_scrollback(0);
    }

    /// Render `id`'s current screen as `(rows_of_cells, cursor)`.
    ///
    /// Returns owned rows rather than a borrow of the emulator: the render pass
    /// must not hold the parser's lock while the reader thread wants it.
    pub fn screen_rows(&self, id: &str) -> Option<ScreenSnapshot> {
        let sessions = self.inner.sessions.lock().unwrap();
        let session = sessions.iter().find(|s| s.row.id == id)?;
        let parser = session.screen.lock().unwrap();
        let screen = parser.screen();
        let (rows, cols) = screen.size();
        let cells = (0..rows)
            .map(|row| {
                (0..cols)
                    .map(|col| {
                        screen
                            .cell(row, col)
                            .map(|cell| ScreenCell {
                                text: {
                                    let contents = cell.contents();
                                    if contents.is_empty() {
                                        " ".to_string()
                                    } else {
                                        contents
                                    }
                                },
                                fg: cell.fgcolor(),
                                bg: cell.bgcolor(),
                                bold: cell.bold(),
                                italic: cell.italic(),
                                underline: cell.underline(),
                                inverse: cell.inverse(),
                            })
                            .unwrap_or_default()
                    })
                    .collect()
            })
            .collect();
        Some(ScreenSnapshot {
            cells,
            cursor: screen.cursor_position(),
            hide_cursor: screen.hide_cursor(),
        })
    }

    /// Resize a session's PTY and emulator to `cols` x `rows`.
    ///
    /// Both must move together: the child reflows to the PTY size, so an
    /// emulator of a different size would render a torn screen.
    pub fn resize(&self, id: &str, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            return;
        }
        let sessions = self.inner.sessions.lock().unwrap();
        let Some(session) = sessions.iter().find(|s| s.row.id == id) else {
            return;
        };
        {
            let mut parser = session.screen.lock().unwrap();
            if parser.screen().size() == (rows, cols) {
                return; // already correct — skip the SIGWINCH storm
            }
            parser.set_size(rows, cols);
        }
        let _ = session.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    /// Write raw bytes to a session's PTY — the focused pane's keystrokes.
    pub fn write(&self, id: &str, bytes: &[u8]) -> Result<(), String> {
        let mut sessions = self.inner.sessions.lock().unwrap();
        let Some(session) = sessions.iter_mut().find(|s| s.row.id == id) else {
            return Err(format!("no session {id}"));
        };
        if !session.row.state.is_running() {
            return Err(format!("{id} has exited"));
        }
        use std::io::Write as _;
        session
            .writer
            .write_all(bytes)
            .and_then(|()| session.writer.flush())
            .map_err(|err| format!("{id}: {err}"))
    }
}

mod types;
pub use types::ScreenCell;
pub use types::ScreenSnapshot;
