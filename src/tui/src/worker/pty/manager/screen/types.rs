//! Data types for the `screen` module.
#[allow(unused_imports)]
use super::*;
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
