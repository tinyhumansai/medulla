//! Pointer text selection: highlighting the swept block and copying it.
//!
//! The selection is read back out of the *rendered buffer* rather than out of
//! whatever model produced it. That is what makes it work everywhere — a
//! transcript, a panel of counters, a nav menu, the status line — without every
//! pane having to expose "the text at row N". What you see is what you copy, in
//! the tmux sense.
//!
//! Consequently both halves of the job run at draw time, after every widget has
//! painted: copy first if the button has been released, then highlight.

use ratatui::Frame;

use super::super::types::App;

impl App {
    /// Copy the selected cells if the drag has ended, then invert them.
    ///
    /// Called last in `App::draw` so it paints over finished widgets instead of
    /// being overdrawn by them.
    pub(super) fn paint_selection(&mut self, f: &mut Frame) {
        let Some((left, top, right, bottom)) = self.selection else {
            return;
        };
        if self.copy_selection {
            self.copy_selection = false;
            let text = selected_text(f, left, top, right, bottom);
            // An empty block is a mis-drag, not a copy: replacing the clipboard
            // with whitespace would lose whatever the operator had in it.
            if !text.trim().is_empty() {
                self.copy_line("selection", &text);
            }
            // The highlight goes with the copy. Leaving it up reads as "still
            // selecting", and the next click would have to clear it anyway.
            self.selection = None;
            return;
        }
        let style = self.theme.selection();
        let buffer = f.buffer_mut();
        let area = buffer.area;
        for y in top..=bottom.min(area.bottom().saturating_sub(1)) {
            for x in left..=right.min(area.right().saturating_sub(1)) {
                if let Some(cell) = buffer.cell_mut((x, y)) {
                    cell.set_style(style);
                }
            }
        }
    }
}

/// Read the selected block out of the rendered buffer, one line per row.
///
/// Trailing blanks are trimmed per row: a block dragged across a pane picks up
/// the padding each line was rendered with, and pasting that back into a shell
/// or an editor is noise.
fn selected_text(f: &mut Frame, left: u16, top: u16, right: u16, bottom: u16) -> String {
    let buffer = f.buffer_mut();
    let area = buffer.area;
    let mut rows = Vec::new();
    for y in top..=bottom.min(area.bottom().saturating_sub(1)) {
        let mut row = String::new();
        for x in left..=right.min(area.right().saturating_sub(1)) {
            if let Some(cell) = buffer.cell((x, y)) {
                row.push_str(cell.symbol());
            }
        }
        rows.push(row.trim_end().to_string());
    }
    rows.join("\n")
}
