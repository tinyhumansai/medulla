//! One session's emulator surface: what the UI renders, and the cheap text
//! projections the injector matches against.

use super::super::cell_text::CellText;
use super::super::manager::{ScreenCell, ScreenSnapshot};
use super::super::sync::lock;
use super::SessionHandle;

/// What an empty cell renders as. A `const` so the common case is a copy of a
/// fixed 16-byte value rather than a fresh construction per cell.
const BLANK: CellText = CellText::blank();

impl SessionHandle {
    /// The harness's current human-facing thread name, when it advertises one.
    ///
    /// Codex and Claude Code update the terminal window title after `/rename`.
    /// Reading that standard PTY signal keeps the rail live without coupling it
    /// to either provider's private transcript format.
    pub fn thread_name(&self) -> Option<String> {
        lock(&self.cold).thread_name.clone()
    }

    /// Sample the live screen text and audible-bell counter for classification.
    ///
    /// Restores the operator's scrollback offset before returning, so a
    /// background poll never moves the pane they are reading.
    pub(in super::super) fn attention_sample(&self) -> (String, usize) {
        let mut parser = lock(&self.screen);
        let held = parser.screen().scrollback();
        parser.set_scrollback(0);
        let sample = (
            parser.screen().contents(),
            parser.screen().audible_bell_count(),
        );
        parser.set_scrollback(held);
        sample
    }

    /// The emulator's current audible-bell counter.
    pub(in super::super) fn bell_count(&self) -> usize {
        lock(&self.screen).screen().audible_bell_count()
    }

    /// Whether the child has turned bracketed-paste mode on (DECSET 2004).
    pub(in super::super) fn bracketed_paste(&self) -> bool {
        lock(&self.screen).screen().bracketed_paste()
    }

    /// Which mouse protocol the child has turned on, if any.
    pub(in super::super) fn mouse_protocol(
        &self,
    ) -> (vt100::MouseProtocolMode, vt100::MouseProtocolEncoding) {
        let parser = lock(&self.screen);
        let screen = parser.screen();
        (
            screen.mouse_protocol_mode(),
            screen.mouse_protocol_encoding(),
        )
    }

    /// Whether the child enabled xterm alternate-scroll mode (DECSET 1007).
    pub(in super::super) fn alternate_scroll(&self) -> bool {
        lock(&self.modes).alternate_scroll
    }

    /// Take the clipboard write the child asked for, if it has asked since the
    /// last call.
    ///
    /// Taking rather than reading, so one copy is forwarded once however often
    /// this is polled — the reader thread calls it after every drain.
    pub(in super::super) fn take_clipboard(&self) -> Option<String> {
        lock(&self.modes).clipboard.take()
    }

    /// Feed bytes from the pty into the emulator.
    pub(in super::super) fn process(&self, bytes: &[u8]) {
        {
            let mut modes = lock(&self.modes);
            let mut parser = std::mem::take(&mut modes.parser);
            for &byte in bytes {
                parser.advance(&mut *modes, byte);
                // Run in parallel with `vte`'s own OSC dispatch above, which
                // truncates a payload past 1024 bytes; a complete capture
                // here supersedes whatever that produced for the same write.
                // See `super::osc52` for why both exist.
                if let Some(text) = modes.osc52.advance(byte) {
                    modes.clipboard = Some(text);
                }
            }
            modes.parser = parser;
        }
        let thread_name = {
            let mut parser = lock(&self.screen);
            parser.process(bytes);
            let title = parser.screen().title().trim().to_string();
            (!title.is_empty()).then_some(title)
        };
        lock(&self.cold).thread_name = thread_name;
    }

    /// Move the emulator's scrollback by `rows`, towards the history when `up`.
    ///
    /// **Bounded to one screenful**, and not by choice: `vt100` 0.15's
    /// `visible_rows` computes `visible_row_count - offset`, so any offset
    /// larger than the screen height underflows and panics inside the crate.
    /// Clamping here is the fix available from outside it.
    pub(in super::super) fn scroll_history(&self, rows: usize, up: bool) -> usize {
        let mut parser = lock(&self.screen);
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
        parser.screen().scrollback()
    }

    /// Snap the emulator back to the live screen.
    pub(in super::super) fn scroll_to_live(&self) {
        lock(&self.screen).set_scrollback(0);
    }

    /// The last `max` non-blank lines of the screen, oldest first.
    ///
    /// What a handoff brief carries: enough of what the operator was doing for
    /// the orchestrator to continue the thread rather than restart it. Plain
    /// text, not cells — nobody downstream renders it, they read it.
    ///
    /// Reads at scrollback offset 0 and puts the operator's offset back
    /// afterwards, so capturing a brief does not move the pane they are looking
    /// at. Only this session's emulator lock is taken, so a brief captured on
    /// one harness never stalls another's reader thread.
    ///
    /// **Bounded to one screenful.** `vt100` 0.15's `visible_rows` underflows
    /// for any offset past the screen height — see
    /// [`scroll_history`](Self::scroll_history) — so walking the full retained
    /// history would panic the TUI. One screen is what a brief needs anyway; the
    /// harness's own transcript is where deep history lives.
    pub(in super::super) fn tail_lines(&self, max: usize) -> Vec<String> {
        let contents = {
            let mut parser = lock(&self.screen);
            let held = parser.screen().scrollback();
            parser.set_scrollback(0);
            let contents = parser.screen().contents();
            parser.set_scrollback(held);
            contents
        };

        let mut lines: Vec<String> = contents
            .lines()
            .map(|line| line.trim_end().to_string())
            .collect();
        // A harness pane is mostly empty space below the prompt, so the tail is
        // otherwise all blanks and the brief carries nothing.
        while lines.last().is_some_and(|line| line.is_empty()) {
            lines.pop();
        }
        if lines.len() > max {
            lines.drain(..lines.len() - max);
        }
        lines
    }

    /// Render the current screen as `(rows_of_cells, cursor)`.
    ///
    /// Owned, so the render pass never holds the emulator's lock.
    ///
    /// The per-cell allocation that remains is `vt100`'s, not ours:
    /// `Cell::contents` builds a fresh `String` on every call and there is no
    /// borrowing accessor in 0.15. What [`CellText`] and `has_contents` buy is
    /// that a **blank** cell now costs nothing at all — no `contents` call, no
    /// allocation — and a harness screen is mostly blank. The text a cell does
    /// hold is copied inline and vt100's temporary is dropped immediately, so a
    /// snapshot no longer holds thousands of live heap strings either.
    pub(in super::super) fn snapshot(&self) -> ScreenSnapshot {
        let parser = lock(&self.screen);
        let screen = parser.screen();
        let (rows, cols) = screen.size();
        let cells = (0..rows)
            .map(|row| {
                let mut line = Vec::with_capacity(usize::from(cols));
                for col in 0..cols {
                    line.push(match screen.cell(row, col) {
                        Some(cell) => ScreenCell {
                            text: if cell.has_contents() {
                                CellText::from(cell.contents().as_str())
                            } else {
                                BLANK
                            },
                            fg: cell.fgcolor(),
                            bg: cell.bgcolor(),
                            bold: cell.bold(),
                            italic: cell.italic(),
                            underline: cell.underline(),
                            inverse: cell.inverse(),
                        },
                        None => ScreenCell::default(),
                    });
                }
                line
            })
            .collect();
        ScreenSnapshot {
            cells,
            cursor: screen.cursor_position(),
            hide_cursor: screen.hide_cursor(),
        }
    }

    /// Write the screen's plain text into `out`, one line per row.
    ///
    /// For recognising *what is on* the screen rather than rendering it. The
    /// caller supplies the buffer so a poller can reuse one: this is called at
    /// 40 Hz per starting session while a prompt is being injected, and building
    /// a full [`ScreenSnapshot`] and then joining it — which is what that path
    /// used to do, four times per tick — was the single largest source of
    /// allocation churn during a fan-out.
    pub(in super::super) fn text_into(&self, out: &mut String) {
        out.clear();
        let parser = lock(&self.screen);
        let screen = parser.screen();
        let (rows, cols) = screen.size();
        for row in 0..rows {
            if row > 0 {
                out.push('\n');
            }
            for col in 0..cols {
                // `has_contents` first: `Cell::contents` allocates a `String`
                // every time it is called, and most of a screen is blank.
                match screen.cell(row, col) {
                    Some(cell) if cell.has_contents() => out.push_str(&cell.contents()),
                    _ => out.push(' '),
                }
            }
        }
    }

    /// Write the screen into `out` with whitespace removed and case folded.
    ///
    /// The form [`super::super::inject`] counts needles in. Produced in one pass
    /// straight from the emulator, where it used to be a snapshot, then a join,
    /// then a second full-screen string per `occurrences` call.
    pub(in super::super) fn squashed_text_into(&self, out: &mut String) {
        out.clear();
        let parser = lock(&self.screen);
        let screen = parser.screen();
        let (rows, cols) = screen.size();
        for row in 0..rows {
            for col in 0..cols {
                // A blank cell contributes nothing to the squashed form, so
                // skipping it here avoids `contents`' allocation entirely — and
                // most of a screen is blank.
                let Some(cell) = screen.cell(row, col).filter(|cell| cell.has_contents()) else {
                    continue;
                };
                for ch in cell.contents().chars() {
                    if ch.is_whitespace() {
                        continue;
                    }
                    for lowered in ch.to_lowercase() {
                        out.push(lowered);
                    }
                }
            }
        }
    }
}
