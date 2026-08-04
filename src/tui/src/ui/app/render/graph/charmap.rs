//! A character-cell drawing surface for the graph.
//!
//! Earlier versions of this panel drew into braille and half-block sub-cell
//! grids for the extra resolution. They looked like pixel art — fine on some
//! terminals, a smear of dotted noise on others, since how those glyphs render
//! depends entirely on the font. This draws ordinary line and marker characters
//! instead, one per cell: less resolution, but it is crisp on every terminal and
//! it matches the glyph vocabulary the rest of the TUI already uses.
//!
//! Coordinates are in *canvas units*: one unit is one column wide and half a row
//! tall, which is roughly square in a normal terminal font. Keeping the vertical
//! unit at half a row means the layout still has fine vertical positioning even
//! though drawing lands on whole cells.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;

/// A color as linear RGB in `0..=255` per channel.
pub(crate) type Rgb = (f64, f64, f64);

/// Canvas units per terminal cell, horizontally.
pub(crate) const UNITS_W: usize = 1;
/// Canvas units per terminal cell, vertically.
pub(crate) const UNITS_H: usize = 2;

/// What a cell holds, so a later draw knows whether it may overwrite it.
///
/// The graph is drawn back to front — edges, then the packets riding them, then
/// the nodes — and a node must never be hidden by a line that happens to pass
/// through it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Layer {
    /// A connection between two nodes.
    Edge,
    /// The moving highlight travelling an edge.
    Packet,
    /// A node marker.
    Node,
}

/// One drawn cell.
#[derive(Debug, Clone, Copy)]
struct Mark {
    /// The character to print.
    glyph: char,
    /// Its foreground color.
    color: Rgb,
    /// What drew it, which decides whether the next draw may replace it.
    layer: Layer,
    /// Whether to print it bold.
    bold: bool,
}

/// A grid of drawn characters, blitted into the frame as a widget.
pub(crate) struct CharMap {
    /// Width in cells.
    cols: usize,
    /// Height in cells.
    rows: usize,
    /// Row-major cells; `None` is untouched and shows the panel behind.
    cells: Vec<Option<Mark>>,
}

impl CharMap {
    /// Allocate an empty surface covering `area`.
    pub(crate) fn for_area(area: Rect) -> Self {
        let (cols, rows) = (area.width as usize, area.height as usize);
        CharMap {
            cols,
            rows,
            cells: vec![None; cols * rows],
        }
    }

    /// Width in canvas units.
    pub(crate) fn width(&self) -> usize {
        self.cols * UNITS_W
    }

    /// Height in canvas units.
    pub(crate) fn height(&self) -> usize {
        self.rows * UNITS_H
    }

    /// Draw an edge as a right-angled elbow: out horizontally from `from`, a
    /// vertical run halfway across, then horizontally into `to`.
    ///
    /// Straight runs and square corners, never diagonals. A terminal cell is
    /// twice as tall as it is wide, so a diagonal drawn with `╱`/`╲` steps at
    /// whatever angle the aspect ratio dictates rather than the angle the edge
    /// actually has, and a fan of them reads as a mess of loose strokes. Two
    /// axis-aligned runs and a corner say "these two are connected" without
    /// pretending to an angle the character grid cannot draw.
    ///
    /// `bias` picks where across the gap the vertical run happens, so a bundle
    /// of edges heading the same way can be spread out instead of stacking
    /// every riser into one column.
    pub(crate) fn elbow(
        &mut self,
        from: (f64, f64),
        to: (f64, f64),
        bias: f64,
        colors: (Rgb, Rgb),
    ) {
        let path = elbow_path(from, to, bias);
        let total = path_length(&path).max(f64::EPSILON);
        let mut travelled = 0.0;
        for pair in path.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let span = distance(a, b);
            // A run of no length has no direction to draw, and guessing one
            // stamps a stray glyph on the cell the two live runs share.
            if span <= f64::EPSILON {
                continue;
            }
            // Step in half-units so a vertical run, whose cells are two units
            // apart, cannot skip one.
            let steps = (span * 2.0).ceil().max(1.0);
            for i in 0..=(steps as usize) {
                let along = i as f64 / steps;
                let color = mix(colors.0, colors.1, (travelled + span * along) / total);
                self.put(
                    (a.0 + (b.0 - a.0) * along, a.1 + (b.1 - a.1) * along),
                    Mark {
                        glyph: if (b.0 - a.0).abs() > f64::EPSILON {
                            '─'
                        } else {
                            '│'
                        },
                        color,
                        layer: Layer::Edge,
                        bold: false,
                    },
                );
            }
            travelled += span;
        }
        // Corners last, so they are not overwritten by the runs meeting there.
        for (corner, glyph) in [
            (path[1], corner_glyph(path[0], path[1], path[2])),
            (path[2], corner_glyph(path[1], path[2], path[3])),
        ] {
            if let Some(glyph) = glyph {
                // Written without merging: the two runs meeting here have
                // already put a `─` and a `│` in this cell, and merging would
                // turn every single corner in the graph into a `┼`.
                self.write(
                    corner,
                    Mark {
                        glyph,
                        color: mix(colors.0, colors.1, 0.5),
                        layer: Layer::Edge,
                        bold: false,
                    },
                    false,
                );
            }
        }
    }

    /// Brighten the cell a packet currently occupies, keeping whatever glyph the
    /// edge already put there so the line stays unbroken.
    pub(crate) fn packet(&mut self, at: (f64, f64), color: Rgb) {
        let Some((col, row)) = self.cell_of(at) else {
            return;
        };
        let glyph = self.cells[row * self.cols + col]
            .map(|mark| mark.glyph)
            .unwrap_or('·');
        self.put(
            at,
            Mark {
                glyph,
                color,
                layer: Layer::Packet,
                bold: true,
            },
        );
    }

    /// Place a node's marker glyph.
    pub(crate) fn node(&mut self, at: (f64, f64), glyph: char, color: Rgb, bold: bool) {
        self.put(
            at,
            Mark {
                glyph,
                color,
                layer: Layer::Node,
                bold,
            },
        );
    }

    /// Write `mark` at a canvas position, unless something at least as important
    /// is already there.
    ///
    /// Two edges crossing at right angles become a `┼` rather than one erasing
    /// the other, which is what makes a dense fan still look like wiring.
    fn put(&mut self, at: (f64, f64), mark: Mark) {
        self.write(at, mark, true);
    }

    /// Write `mark`, optionally merging its glyph with an edge already in the
    /// cell. Anything from a higher layer always wins.
    fn write(&mut self, at: (f64, f64), mut mark: Mark, merge: bool) {
        let Some((col, row)) = self.cell_of(at) else {
            return;
        };
        let slot = &mut self.cells[row * self.cols + col];
        if let Some(held) = *slot {
            if held.layer > mark.layer {
                return;
            }
            if merge && held.layer == Layer::Edge && mark.layer == Layer::Edge {
                mark.glyph = crossing(held.glyph, mark.glyph);
            }
        }
        *slot = Some(mark);
    }

    /// The cell a canvas position falls in, or `None` if it is off the surface.
    ///
    /// Off-surface positions are dropped rather than clamped: clamping would
    /// smear anything running off an edge along that edge.
    fn cell_of(&self, (x, y): (f64, f64)) -> Option<(usize, usize)> {
        let (col, row) = ((x / UNITS_W as f64).round(), (y / UNITS_H as f64).round());
        if col < 0.0 || row < 0.0 || col as usize >= self.cols || row as usize >= self.rows {
            return None;
        }
        Some((col as usize, row as usize))
    }
}

/// Blend `b` into `a` by `amount` in `0..=1`.
pub(crate) fn mix(a: Rgb, b: Rgb, amount: f64) -> Rgb {
    (
        a.0 + (b.0 - a.0) * amount,
        a.1 + (b.1 - a.1) * amount,
        a.2 + (b.2 - a.2) * amount,
    )
}

/// Scale a color's brightness, saturating at white.
pub(crate) fn scale(color: Rgb, factor: f64) -> Rgb {
    (
        (color.0 * factor).clamp(0.0, 255.0),
        (color.1 * factor).clamp(0.0, 255.0),
        (color.2 * factor).clamp(0.0, 255.0),
    )
}

/// Convert a canvas color to a terminal color.
pub(crate) fn rgb(color: Rgb) -> Color {
    Color::Rgb(color.0 as u8, color.1 as u8, color.2 as u8)
}

/// The four corners of the elbow route between two points: out from `from`,
/// across at `bias` of the way over, and in to `to`.
///
/// Exposed so packets can be placed along exactly the path that was drawn.
pub(crate) fn elbow_path(from: (f64, f64), to: (f64, f64), bias: f64) -> [(f64, f64); 4] {
    // Two nodes less than a row apart are on the same row once drawn, so the
    // riser between them would be shorter than one cell: both of its corners
    // would land in the same place and merge into a meaningless junction. Snap
    // to a straight run instead — which is what the edge looks like anyway.
    let to = if (to.1 - from.1).abs() < UNITS_H as f64 * 0.75 {
        (to.0, from.1)
    } else {
        to
    };
    let turn = from.0 + (to.0 - from.0) * bias.clamp(0.15, 0.85);
    [from, (turn, from.1), (turn, to.1), to]
}

/// The point `t` of the way along `path`, by arc length.
pub(crate) fn point_along(path: &[(f64, f64); 4], t: f64) -> (f64, f64) {
    let total = path_length(path);
    if total <= f64::EPSILON {
        return path[0];
    }
    let mut target = t.clamp(0.0, 1.0) * total;
    for pair in path.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let span = distance(a, b);
        if target <= span || span <= f64::EPSILON {
            let along = if span > f64::EPSILON {
                target / span
            } else {
                0.0
            };
            return (a.0 + (b.0 - a.0) * along, a.1 + (b.1 - a.1) * along);
        }
        target -= span;
    }
    path[3]
}

/// Total arc length of a path.
fn path_length(path: &[(f64, f64); 4]) -> f64 {
    path.windows(2).map(|pair| distance(pair[0], pair[1])).sum()
}

/// Straight-line distance between two canvas points.
fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
}

/// The box-drawing corner joining the run `before -> at` to `at -> after`, or
/// `None` when the path does not actually turn there.
fn corner_glyph(before: (f64, f64), at: (f64, f64), after: (f64, f64)) -> Option<char> {
    let entering_x = at.0 - before.0;
    let leaving_x = after.0 - at.0;
    let entering_y = at.1 - before.1;
    let leaving_y = after.1 - at.1;
    // Canvas y grows downward, so a positive delta descends the screen.
    match (entering_x, entering_y, leaving_x, leaving_y) {
        // Arriving horizontally, leaving vertically.
        (dx, _, _, dy) if dx > 0.0 && dy > 0.0 => Some('┐'),
        (dx, _, _, dy) if dx > 0.0 && dy < 0.0 => Some('┘'),
        (dx, _, _, dy) if dx < 0.0 && dy > 0.0 => Some('┌'),
        (dx, _, _, dy) if dx < 0.0 && dy < 0.0 => Some('└'),
        // Arriving vertically, leaving horizontally.
        (_, dy, dx, _) if dy > 0.0 && dx > 0.0 => Some('└'),
        (_, dy, dx, _) if dy > 0.0 && dx < 0.0 => Some('┘'),
        (_, dy, dx, _) if dy < 0.0 && dx > 0.0 => Some('┌'),
        (_, dy, dx, _) if dy < 0.0 && dx < 0.0 => Some('┐'),
        _ => None,
    }
}

/// Which sides of its cell a box-drawing glyph connects to, as `UP|DOWN|LEFT|
/// RIGHT` bits.
const fn connections(glyph: char) -> u8 {
    const UP: u8 = 1;
    const DOWN: u8 = 2;
    const LEFT: u8 = 4;
    const RIGHT: u8 = 8;
    match glyph {
        '─' => LEFT | RIGHT,
        '│' => UP | DOWN,
        '┌' => DOWN | RIGHT,
        '┐' => DOWN | LEFT,
        '└' => UP | RIGHT,
        '┘' => UP | LEFT,
        '├' => UP | DOWN | RIGHT,
        '┤' => UP | DOWN | LEFT,
        '┬' => DOWN | LEFT | RIGHT,
        '┴' => UP | LEFT | RIGHT,
        '┼' => UP | DOWN | LEFT | RIGHT,
        _ => 0,
    }
}

/// The glyph for a cell two edges both pass through.
///
/// Merging the two glyphs' connections and looking the result back up is what
/// turns a corner landing on a straight run into a proper `┬`, and two runs
/// crossing into a `┼`, rather than one silently erasing the other.
fn crossing(held: char, incoming: char) -> char {
    let wanted = connections(held) | connections(incoming);
    if wanted == 0 {
        return incoming;
    }
    ['─', '│', '┌', '┐', '└', '┘', '├', '┤', '┬', '┴', '┼']
        .into_iter()
        .find(|&glyph| connections(glyph) == wanted)
        .unwrap_or(incoming)
}

impl Widget for &CharMap {
    /// Blit every drawn cell into `area`, leaving untouched cells alone so the
    /// panel behind shows through.
    fn render(self, area: Rect, buf: &mut Buffer) {
        for row in 0..area.height.min(self.rows as u16) {
            for col in 0..area.width.min(self.cols as u16) {
                let Some(mark) = self.cells[row as usize * self.cols + col as usize] else {
                    continue;
                };
                let Some(cell) = buf.cell_mut((area.x + col, area.y + row)) else {
                    continue;
                };
                let mut style = Style::default().fg(rgb(mark.color));
                if mark.bold {
                    style = style.add_modifier(Modifier::BOLD);
                }
                cell.set_char(mark.glyph).set_style(style);
            }
        }
    }
}
