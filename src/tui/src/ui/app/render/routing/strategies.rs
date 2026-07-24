//! Operator-selectable model and worker routing strategy surface.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::super::super::types::App;

impl App {
    /// Draw the strategy chooser shell.
    pub(super) fn draw_strategies(&self, f: &mut Frame, area: Rect) {
        let lines = vec![
            TLine::from("Balanced"),
            TLine::from(""),
            TLine::from(Span::styled(
                "Strategy controls will choose how Medulla balances quality, cost, and available seats.",
                Style::default().add_modifier(Modifier::DIM),
            )),
        ];
        f.render_widget(
            Paragraph::new(Text::from(lines)).block(self.panel("Strategies")),
            area,
        );
    }
}
