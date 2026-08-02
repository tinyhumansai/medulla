//! The boundary between the emulator's screen and the wire model.
//!
//! The SDK's screen protocol is deliberately free of `vt100` — that is what
//! keeps the diff, the fold and the codec testable without a pty. So the
//! translation lives here, in the crate that already owns the emulator, and is
//! the only place the two vocabularies meet.
//!
//! It is a pure function of a [`ScreenSnapshot`], so it can be exercised against
//! literal cells with no child process involved.

use medulla::tinyplace::{coalesce_runs, Color, RunStyle, ScreenGrid, ScreenRun};

use super::super::pty::{ScreenCell, ScreenSnapshot};

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
        attrs |= medulla::tinyplace::ATTR_BOLD;
    }
    if cell.italic {
        attrs |= medulla::tinyplace::ATTR_ITALIC;
    }
    if cell.underline {
        attrs |= medulla::tinyplace::ATTR_UNDERLINE;
    }
    if cell.inverse {
        attrs |= medulla::tinyplace::ATTR_INVERSE;
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
