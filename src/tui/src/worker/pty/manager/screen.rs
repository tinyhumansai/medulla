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

/// One rendered terminal cell.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScreenCell {
    /// The cell's text (a space when blank).
    pub text: String,
    /// Foreground color.
    pub fg: vt100::Color,
    /// Background color.
    pub bg: vt100::Color,
    /// Whether the cell is bold.
    pub bold: bool,
    /// Whether the cell is italic.
    pub italic: bool,
    /// Whether the cell is underlined.
    pub underline: bool,
    /// Whether foreground/background are swapped.
    pub inverse: bool,
}

/// An owned copy of a session's screen, safe to render without holding the
/// emulator's lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenSnapshot {
    /// Rows of cells, top to bottom.
    pub cells: Vec<Vec<ScreenCell>>,
    /// The cursor's `(row, col)`.
    pub cursor: (u16, u16),
    /// Whether the harness has hidden its cursor.
    pub hide_cursor: bool,
}
