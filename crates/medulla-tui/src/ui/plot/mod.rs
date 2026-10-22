//! Reusable btop-style graph and meter widgets for the terminal UI.

mod graph;
mod meter;
mod symbols;

#[cfg(test)]
mod tests;

pub use graph::Graph;
pub use meter::Meter;
pub use symbols::Symbol;

use ratatui::style::Color;

/// A color gradient sampled from 0 to 100 inclusive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gradient {
    stops: Vec<Color>,
}

impl Gradient {
    /// Build a gradient from any number of color stops.
    ///
    /// Stops are spread evenly from 0 through 100. RGB stops are interpolated
    /// channel by channel; other Ratatui colors switch at the midpoint between
    /// stops. An empty list behaves as a solid white gradient.
    pub fn new(stops: impl Into<Vec<Color>>) -> Self {
        Self {
            stops: stops.into(),
        }
    }

    pub(super) fn at(&self, percent: f32) -> Color {
        match self.stops.as_slice() {
            [] => Color::White,
            [color] => *color,
            stops => {
                let position = percent.clamp(0.0, 100.0) / 100.0 * (stops.len() - 1) as f32;
                let low = position.floor() as usize;
                let high = (low + 1).min(stops.len() - 1);
                let amount = position - low as f32;
                blend(stops[low], stops[high], amount)
            }
        }
    }
}

fn blend(from: Color, to: Color, amount: f32) -> Color {
    match (from, to) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => Color::Rgb(
            mix(r1, r2, amount),
            mix(g1, g2, amount),
            mix(b1, b2, amount),
        ),
        (from, to) => {
            if amount < 0.5 {
                from
            } else {
                to
            }
        }
    }
}

fn mix(from: u8, to: u8, amount: f32) -> u8 {
    (f32::from(from) + (f32::from(to) - f32::from(from)) * amount).round() as u8
}
