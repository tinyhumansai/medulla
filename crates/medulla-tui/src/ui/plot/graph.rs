//! Paired-sample history graph widget.

use super::{Gradient, Symbol};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

/// Draw a percentage history, packing two samples into each terminal cell.
///
/// Values are clamped to `0..=100`. The default symbol set is Braille, matching
/// the resource plot decision in the Resources specification.
pub struct Graph<'a> {
    values: &'a [f32],
    symbol: Symbol,
    invert: bool,
    no_zero: bool,
    gradient: Option<&'a Gradient>,
}

impl<'a> Graph<'a> {
    /// Create a graph over percentage samples.
    pub fn new(values: &'a [f32]) -> Self {
        Self {
            values,
            symbol: Symbol::default(),
            invert: false,
            no_zero: false,
            gradient: None,
        }
    }

    /// Choose the glyph family. Defaults to [`Symbol::Braille`].
    pub fn symbol(mut self, symbol: Symbol) -> Self {
        self.symbol = symbol;
        self
    }

    /// Draw with the descending btop lookup table.
    pub fn invert(mut self, invert: bool) -> Self {
        self.invert = invert;
        self
    }

    /// Keep zero samples visible as a one-dot baseline.
    pub fn no_zero(mut self, no_zero: bool) -> Self {
        self.no_zero = no_zero;
        self
    }

    /// Color the graph with this gradient.
    pub fn gradient(mut self, gradient: &'a Gradient) -> Self {
        self.gradient = Some(gradient);
        self
    }
}

impl Widget for Graph<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                buffer[(x, y)] = ratatui::buffer::Cell::default()
                    .set_char(' ')
                    .set_style(Style::default())
                    .clone();
            }
        }
        if self.values.is_empty() {
            return;
        }
        let width = area.width as usize;
        let height = area.height as usize;
        let first = self.values.len().saturating_sub(width.saturating_mul(2));
        let values = &self.values[first..];
        let odd = values.len() % 2 == 1;
        let cell_count = values.len().div_ceil(2);
        let first_column = width - cell_count;
        let one_row_bias = if height == 1 { 0.3 } else { 0.1 };

        for cell_index in 0..cell_count {
            let (previous, current) = if odd && cell_index == 0 {
                (0.0, values[0])
            } else {
                let value_index = cell_index * 2 - usize::from(odd);
                (values[value_index], values[value_index + 1])
            };
            let column = first_column + cell_index;
            for row in 0..height {
                let high = if height > 1 {
                    (100.0 * (height - row) as f32 / height as f32).round()
                } else {
                    100.0
                };
                let low = if height > 1 {
                    (100.0 * (height - row - 1) as f32 / height as f32).round()
                } else {
                    0.0
                };
                let bottom = row == height - 1;
                let left_no_zero =
                    self.no_zero && bottom && !(odd && cell_index == 0 && first == 0);
                let left_level = level(previous, low, high, one_row_bias, left_no_zero);
                let right_level = level(current, low, high, one_row_bias, self.no_zero && bottom);
                let glyph = self.symbol.glyph(self.invert, left_level, right_level);
                let color_percent = if height == 1 {
                    previous.max(current).clamp(0.0, 100.0)
                } else {
                    high
                };
                let style = self
                    .gradient
                    .map(|gradient| Style::default().fg(gradient.at(color_percent)))
                    .unwrap_or_default();
                buffer[(area.x + column as u16, area.y + row as u16)] =
                    ratatui::buffer::Cell::default()
                        .set_char(glyph)
                        .set_style(style)
                        .clone();
            }
        }
    }
}

fn level(value: f32, low: f32, high: f32, bias: f32, no_zero: bool) -> u8 {
    let value = if value.is_finite() {
        value.clamp(0.0, 100.0)
    } else {
        0.0
    };
    let minimum = u8::from(no_zero);
    if high <= low {
        minimum
    } else if value >= high {
        4
    } else if value <= low {
        minimum
    } else {
        (((value - low) * 4.0 / (high - low) + bias).round() as i32).clamp(i32::from(minimum), 4)
            as u8
    }
}
