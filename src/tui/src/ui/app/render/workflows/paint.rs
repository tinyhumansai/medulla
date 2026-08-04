//! A character grid the graph is painted onto before it is turned into lines.
//!
//! Drawing a DAG directly into ratatui lines does not work: boxes and the wires
//! between them are laid out in two dimensions and emitted in one, and an edge
//! that leaves a node three lanes up has to be written into rows that were
//! finished several nodes ago. So the canvas is painted into a grid first and
//! serialised at the end.
//!
//! The grid does two things a `Vec<String>` cannot:
//!
//! - **Node cells are locked.** A wire routed across the canvas tunnels behind a
//!   box instead of writing over its label, which is what makes long edges
//!   drawable at all.
//! - **Wires merge.** Each painted cell records which of its four sides a line
//!   leaves by, and the box-drawing character is chosen from that set at the
//!   end. Two edges crossing produce `┼`, a branch produces `┬`, and neither has
//!   to be special-cased by the caller.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span};

/// Which sides a wire leaves a cell by. Bit flags, ORed as segments are painted.
const UP: u8 = 1;
const DOWN: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;

/// One character of the canvas.
#[derive(Clone)]
struct Cell {
    /// The character, once decided. Wire cells leave this empty and are
    /// resolved from [`wire`](Self::wire) at the end.
    ch: char,
    /// The sides a wire leaves this cell by.
    wire: u8,
    /// Colour, or `None` for the terminal default.
    color: Option<Color>,
    /// Whether the cell renders dim.
    dim: bool,
    /// Whether the cell renders bold.
    bold: bool,
    /// Whether the cell is reversed — how the cursor marks the selected box.
    selected: bool,
    /// Whether a node owns this cell. Wires never paint over one.
    locked: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            wire: 0,
            color: None,
            dim: false,
            bold: false,
            selected: false,
            locked: false,
        }
    }
}

/// A fixed-size character grid.
pub(super) struct Canvas {
    width: usize,
    height: usize,
    cells: Vec<Cell>,
}

impl Canvas {
    /// An empty canvas of `width` × `height` cells.
    pub(super) fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            cells: vec![Cell::default(); width * height],
        }
    }

    /// The cell at `(x, y)`, when it is on the canvas.
    fn at(&mut self, x: usize, y: usize) -> Option<&mut Cell> {
        (x < self.width && y < self.height).then(|| &mut self.cells[y * self.width + x])
    }

    /// Write `text` starting at `(x, y)`, locking every cell it covers.
    ///
    /// Used for node boxes and their labels. Clipped at the right edge rather
    /// than wrapping, because a label that wrapped would land inside the next
    /// column's node.
    pub(super) fn text(&mut self, x: usize, y: usize, text: &str, style: CellStyle) {
        for (offset, ch) in text.chars().enumerate() {
            let Some(cell) = self.at(x + offset, y) else {
                continue;
            };
            cell.ch = ch;
            cell.color = style.color;
            cell.dim = style.dim;
            cell.bold = style.bold;
            cell.selected = style.selected;
            cell.locked = true;
        }
    }

    /// Paint a horizontal wire from `x0` to `x1` inclusive, on row `y`.
    ///
    /// Direction is inferred from the arguments, so a caller drawing a back edge
    /// passes them reversed and gets the same line.
    pub(super) fn horizontal(&mut self, x0: usize, x1: usize, y: usize, style: CellStyle) {
        let (lo, hi) = (x0.min(x1), x0.max(x1));
        for x in lo..=hi {
            let sides = if x == lo {
                RIGHT
            } else if x == hi {
                LEFT
            } else {
                LEFT | RIGHT
            };
            self.wire(x, y, sides, style);
        }
    }

    /// Paint a vertical wire from `y0` to `y1` inclusive, in column `x`.
    pub(super) fn vertical(&mut self, x: usize, y0: usize, y1: usize, style: CellStyle) {
        let (lo, hi) = (y0.min(y1), y0.max(y1));
        for y in lo..=hi {
            let sides = if y == lo {
                DOWN
            } else if y == hi {
                UP
            } else {
                UP | DOWN
            };
            self.wire(x, y, sides, style);
        }
    }

    /// Join a horizontal run into a vertical one at `(x, y)`.
    ///
    /// Called at the two corners of a routed edge. The corner's own sides are
    /// whatever the two runs already painted, so this only has to make sure both
    /// were recorded — which [`horizontal`](Self::horizontal) and
    /// [`vertical`](Self::vertical) do when their ranges meet at the corner.
    pub(super) fn arrow(&mut self, x: usize, y: usize, glyph: char, style: CellStyle) {
        let Some(cell) = self.at(x, y) else {
            return;
        };
        if cell.locked {
            return;
        }
        cell.ch = glyph;
        cell.wire = 0;
        cell.color = style.color;
        cell.dim = style.dim;
    }

    /// Recolour a wire cell without changing what it draws.
    ///
    /// This is how the flow highlight moves along an edge: the wire keeps the
    /// glyph its routing gave it, so the line stays unbroken, and only the one
    /// cell the packet currently occupies is lit. Node cells and cells no wire
    /// reached are left alone, so a highlight can never appear off its own wire.
    pub(super) fn pulse(&mut self, x: usize, y: usize, color: Color) {
        let Some(cell) = self.at(x, y) else {
            return;
        };
        if cell.locked || cell.wire == 0 {
            return;
        }
        cell.color = Some(color);
        cell.dim = false;
        cell.bold = true;
    }

    /// Record that a wire leaves `(x, y)` by `sides`.
    fn wire(&mut self, x: usize, y: usize, sides: u8, style: CellStyle) {
        let Some(cell) = self.at(x, y) else {
            return;
        };
        // A node owns its cells: a wire crossing one tunnels behind it rather
        // than writing over the label.
        if cell.locked {
            return;
        }
        cell.wire |= sides;
        // The first wire to reach a cell colours it. A crossing keeps the
        // earlier colour rather than flickering between two edges' colours,
        // which is arbitrary either way and stable this way.
        if cell.color.is_none() {
            cell.color = style.color;
            cell.dim = style.dim;
        }
    }

    /// Serialise the grid into one ratatui line per row.
    pub(super) fn into_lines(self) -> Vec<TLine<'static>> {
        let mut lines = Vec::with_capacity(self.height);
        for row in self.cells.chunks(self.width) {
            let mut spans: Vec<Span<'static>> = Vec::new();
            let mut run = String::new();
            let mut run_style: Option<(Option<Color>, bool, bool, bool)> = None;
            for cell in row {
                let ch = if cell.wire != 0 && cell.ch == ' ' {
                    wire_char(cell.wire)
                } else {
                    cell.ch
                };
                let style = (cell.color, cell.dim, cell.bold, cell.selected);
                // Runs of identically styled cells become one span, so a row of
                // canvas is a handful of spans rather than one per column.
                if run_style != Some(style) && !run.is_empty() {
                    spans.push(span(&run, run_style));
                    run.clear();
                }
                run_style = Some(style);
                run.push(ch);
            }
            if !run.is_empty() {
                spans.push(span(&run, run_style));
            }
            lines.push(TLine::from(spans));
        }
        lines
    }
}

/// How a painted cell is styled.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct CellStyle {
    /// Foreground colour, or `None` for the terminal default.
    pub(super) color: Option<Color>,
    /// Whether the cell renders dim.
    pub(super) dim: bool,
    /// Whether the cell renders bold.
    pub(super) bold: bool,
    /// Whether the cell renders reversed.
    pub(super) selected: bool,
}

impl CellStyle {
    /// A plain cell in `color`.
    pub(super) fn colored(color: Color) -> Self {
        Self {
            color: Some(color),
            dim: false,
            bold: false,
            selected: false,
        }
    }

    /// The same style, dimmed.
    pub(super) fn dimmed(self) -> Self {
        Self { dim: true, ..self }
    }

    /// The same style, bold.
    pub(super) fn bold(self) -> Self {
        Self { bold: true, ..self }
    }

    /// The same style, marked as the cursor's.
    pub(super) fn selected(self, selected: bool) -> Self {
        Self { selected, ..self }
    }
}

/// A styled span for one run of identically styled cells.
fn span(text: &str, style: Option<(Option<Color>, bool, bool, bool)>) -> Span<'static> {
    let (color, dim, bold, selected) = style.unwrap_or((None, false, false, false));
    let mut out = Style::default();
    if let Some(color) = color {
        out = out.fg(color);
    }
    if dim {
        out = out.add_modifier(Modifier::DIM);
    }
    if bold {
        out = out.add_modifier(Modifier::BOLD);
    }
    if selected {
        out = out.add_modifier(Modifier::REVERSED);
    }
    Span::styled(text.to_string(), out)
}

/// The box-drawing character for a set of sides.
///
/// Exhaustive over the sixteen combinations rather than assembled, because the
/// half-lines (a wire that starts and stops in one cell) have no obvious
/// composition and are best named.
fn wire_char(sides: u8) -> char {
    match sides {
        s if s == UP | DOWN | LEFT | RIGHT => '┼',
        s if s == UP | DOWN | RIGHT => '├',
        s if s == UP | DOWN | LEFT => '┤',
        s if s == DOWN | LEFT | RIGHT => '┬',
        s if s == UP | LEFT | RIGHT => '┴',
        s if s == UP | DOWN => '│',
        s if s == LEFT | RIGHT => '─',
        s if s == DOWN | RIGHT => '╭',
        s if s == DOWN | LEFT => '╮',
        s if s == UP | RIGHT => '╰',
        s if s == UP | LEFT => '╯',
        s if s == UP || s == DOWN => '│',
        s if s == LEFT || s == RIGHT => '─',
        _ => ' ',
    }
}

#[cfg(test)]
#[path = "paint_tests.rs"]
mod tests;
