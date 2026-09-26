//! Soft-wrap math for the composer: the draft's flat char offsets mapped onto
//! the rows a terminal of a given width actually shows.
//!
//! The draft is one string with embedded hard newlines, but a row of a fixed
//! width holds only so much of it, so a long paragraph occupies several rows
//! without containing a single `\n`. Rendering it a logical line at a time drops
//! everything past the pane's right edge — the part being typed.
//!
//! Every char of the draft belongs to exactly one row here, and rows tile the
//! draft in order. That is what lets the caret be placed by offset arithmetic
//! rather than guessed at: a wrap that trimmed the space it broke on (as
//! [`crate::ui::util::wrap`] does, correctly, for read-only prose) would leave
//! the caret one column off for the rest of the line.

use super::types::VisualRow;

/// Split `text` into the rows a `width`-column pane renders it as.
///
/// Hard newlines always break; beyond that a line breaks after the last space
/// that fits, and a run with no space in it is cut at `width`. The space is kept
/// on the row it ends rather than opening the next one, so no char goes missing
/// and the caret can still sit on it.
///
/// A `width` of zero is treated as one: the caller has a pane too narrow to draw
/// in, and returning no rows at all would lose the caret instead of squeezing
/// it.
pub fn wrap_rows(text: &str, width: usize) -> Vec<VisualRow> {
    let width = width.max(1);
    let mut rows = Vec::new();
    // Flat char offset of the logical line currently being wrapped.
    let mut line_start = 0usize;
    for line in text.split('\n') {
        let chars: Vec<char> = line.chars().collect();
        let len = chars.len();
        let mut i = 0usize;
        loop {
            let end = if len - i <= width {
                len
            } else {
                match chars[i..i + width].iter().rposition(|&c| c == ' ') {
                    // `+ 1`: break *after* the space, keeping it on this row.
                    Some(pos) => i + pos + 1,
                    // No space to break on — a URL, a path, a wall of hashes.
                    // Cut it, because the alternative is a row that never ends.
                    None => i + width,
                }
            };
            rows.push(VisualRow {
                start: line_start + i,
                text: chars[i..end].iter().collect(),
            });
            i = end;
            if i >= len {
                break;
            }
        }
        // `+ 1` for the newline itself, which belongs to no row.
        line_start += len + 1;
    }
    rows
}

/// Locate the caret within wrapped `rows` as a (row index, column) pair.
///
/// The caret belongs to the last row that starts at or before it, which puts an
/// offset sitting on a hard newline at the end of the row it terminates rather
/// than at the start of the next one — where the operator last typed, not where
/// the next char will land. Returns `(0, 0)` for an empty row set.
pub fn caret_visual(rows: &[VisualRow], cursor: usize) -> (usize, usize) {
    let index = rows
        .iter()
        .rposition(|row| row.start <= cursor)
        .unwrap_or(0);
    let Some(row) = rows.get(index) else {
        return (0, 0);
    };
    let col = cursor
        .saturating_sub(row.start)
        .min(row.text.chars().count());
    (index, col)
}
