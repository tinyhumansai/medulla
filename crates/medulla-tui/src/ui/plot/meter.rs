//! Gradient filled percentage meter widget.

use super::Gradient;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Widget;

/// A width-bounded meter using filled square glyphs.
pub struct Meter<'a> {
    value: f32,
    gradient: &'a Gradient,
    meter_bg: Color,
}

impl<'a> Meter<'a> {
    /// Create a meter for a value in `0..=100`.
    pub fn new(value: f32, gradient: &'a Gradient, meter_bg: Color) -> Self {
        Self {
            value,
            gradient,
            meter_bg,
        }
    }
}

impl Widget for Meter<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let value = if self.value.is_finite() {
            self.value.clamp(0.0, 100.0)
        } else {
            0.0
        };
        let filled = (value * area.width as f32 / 100.0).floor() as usize;
        for column in 0..area.width as usize {
            let color = if column < filled {
                let position = (column + 1) as f32 * 100.0 / area.width as f32;
                self.gradient.at(position)
            } else {
                self.meter_bg
            };
            buffer[(area.x + column as u16, area.y)] = ratatui::buffer::Cell::default()
                .set_char('■')
                .set_style(Style::default().fg(color))
                .clone();
        }
    }
}
