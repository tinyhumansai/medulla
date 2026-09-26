//! The boundary between the emulator's screen and the wire model.
//!
//! The SDK's screen protocol is deliberately free of `vt100` — that is what
//! keeps the diff, the fold and the codec testable without a pty. So the
//! translation lives here, in the crate that already owns the emulator, and is
//! the only place the two vocabularies meet.
//!
//! It is a pure function of a [`ScreenSnapshot`], so it can be exercised against
//! literal cells with no child process involved.

use medulla::protocol::{coalesce_runs, Color, RunStyle, ScreenGrid, ScreenRun};

use super::super::pty::{CellText, ScreenCell, ScreenSnapshot};

/// Map an emulator colour onto the wire's.
///
/// `Default` is carried through as `Default` rather than resolved to a concrete
/// colour: the viewer should inherit its own palette for unstyled text, exactly
/// as the local renderer does.
pub fn wire_color(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Default,
        vt100::Color::Idx(i) => Color::Idx(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// The wire style for one emulator cell.
///
/// Note that `inverse` is carried as a flag rather than applied by swapping
/// foreground and background here. The local renderer swaps them at paint time
/// because terminals disagree about how REVERSED composes with an explicit
/// background — but that is a rendering decision, and baking it into the wire
/// format would leave the viewer unable to tell an inverted cell from a
/// deliberately colour-swapped one.
pub fn wire_style(cell: &ScreenCell) -> RunStyle {
    let mut attrs = 0u8;
    if cell.bold {
        attrs |= medulla::protocol::ATTR_BOLD;
    }
    if cell.italic {
        attrs |= medulla::protocol::ATTR_ITALIC;
    }
    if cell.underline {
        attrs |= medulla::protocol::ATTR_UNDERLINE;
    }
    if cell.inverse {
        attrs |= medulla::protocol::ATTR_INVERSE;
    }
    RunStyle {
        fg: wire_color(cell.fg),
        bg: wire_color(cell.bg),
        attrs,
    }
}

/// Convert one row of cells into coalesced runs.
fn wire_row(cells: &[ScreenCell]) -> Vec<ScreenRun> {
    coalesce_runs(
        cells
            .iter()
            .map(|cell| (cell.text.as_str().to_string(), wire_style(cell))),
    )
}

/// Convert an emulator snapshot into the grid the protocol synchronises.
///
/// Dimensions are taken from the snapshot itself rather than from the session's
/// configured size, so the grid always describes what was actually read — a
/// snapshot taken mid-resize describes the screen it came from, not the one the
/// pty is about to become.
pub fn wire_grid(snapshot: &ScreenSnapshot) -> ScreenGrid {
    let rows = snapshot.cells.len() as u16;
    let cols = snapshot.cells.first().map(|row| row.len()).unwrap_or(0) as u16;
    ScreenGrid {
        cols,
        rows,
        lines: snapshot.cells.iter().map(|row| wire_row(row)).collect(),
        cursor: snapshot.cursor,
        hide_cursor: snapshot.hide_cursor,
    }
}

/// Map a wire colour back onto the emulator's.
///
/// The exact inverse of [`wire_color`]: `Default` stays `Default`, so a remote
/// session's unstyled text inherits the *viewer's* palette rather than the
/// sender's, which is what makes a remote pane look like it belongs in this
/// terminal.
pub fn cell_color(color: &Color) -> vt100::Color {
    match color {
        Color::Default => vt100::Color::Default,
        Color::Idx(i) => vt100::Color::Idx(*i),
        Color::Rgb(r, g, b) => vt100::Color::Rgb(*r, *g, *b),
    }
}

/// Expand one wire run into the terminal cells it stands for.
///
/// A run's text is its cells concatenated, and the split back into cells is by
/// **grapheme cluster** — which is what a terminal cell holds, as
/// [`CellText`](crate::worker::pty::CellText)'s own docs state.
///
/// Splitting by scalar is wrong in more ways than one: `e` followed by a
/// combining acute is one cell that `chars()` yields twice, a ZWJ emoji is
/// several scalars in one cell, and a flag is two regional indicators in one.
/// Every one of those emitted extra cells, shifting the rest of the row and
/// letting the row's `resize` truncate real content off the end.
///
/// Wide glyphs need no handling here: vt100 already represents one as a leading
/// cell plus an empty continuation cell, so the continuation crosses the wire as
/// its own blank and returns as its own cell — synthesising another would double
/// it.
fn cells_of_run(run: &ScreenRun, into: &mut Vec<ScreenCell>) {
    use unicode_segmentation::UnicodeSegmentation;
    let style = &run.style;
    for cluster in run.text.graphemes(true) {
        into.push(ScreenCell {
            text: CellText::from(cluster),
            fg: cell_color(&style.fg),
            bg: cell_color(&style.bg),
            bold: style.attrs & medulla::protocol::ATTR_BOLD != 0,
            italic: style.attrs & medulla::protocol::ATTR_ITALIC != 0,
            underline: style.attrs & medulla::protocol::ATTR_UNDERLINE != 0,
            inverse: style.attrs & medulla::protocol::ATTR_INVERSE != 0,
        });
    }
}

/// Expand a synchronised wire grid back into the snapshot the renderer takes.
///
/// The inverse of [`wire_grid`], and the whole of what lets a session on another
/// machine render like one on this one. Everything downstream of a snapshot —
/// `screen_lines`, cursor placement, the pane's hit map, the scroll gutter —
/// then works on remote and local sessions identically, with no second renderer
/// to keep in step and no `remote` branch threaded through the drawing code.
///
/// Rows are padded to [`ScreenGrid::cols`] with blanks, because
/// [`coalesce_runs`] deliberately drops unstyled trailing blanks on the way out:
/// what the wire carries is a row's *content*, and the renderer wants a
/// rectangle. A row longer than `cols` is truncated for the same reason — the
/// grid's declared width is what the pane was told to draw.
pub fn snapshot_from_grid(grid: &ScreenGrid) -> ScreenSnapshot {
    let width = grid.cols as usize;
    let blank = ScreenCell {
        text: CellText::from(" "),
        fg: vt100::Color::Default,
        bg: vt100::Color::Default,
        bold: false,
        italic: false,
        underline: false,
        inverse: false,
    };
    let mut cells: Vec<Vec<ScreenCell>> = grid
        .lines
        .iter()
        .map(|runs| {
            let mut row: Vec<ScreenCell> = Vec::with_capacity(width);
            for run in runs {
                cells_of_run(run, &mut row);
            }
            row.resize(width, blank.clone());
            row
        })
        .collect();
    // A grid that declares more rows than it carries lines is padded rather than
    // rejected: the alternative is a pane that shrinks whenever a frame arrives
    // describing a screen whose last rows happen to be empty.
    cells.resize(grid.rows as usize, vec![blank.clone(); width]);
    ScreenSnapshot {
        cells,
        cursor: grid.cursor,
        hide_cursor: grid.hide_cursor,
    }
}
