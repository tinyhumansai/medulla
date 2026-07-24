//! Operator-selectable model and worker routing strategy surface.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::super::super::types::{App, ROUTING_STRATEGIES};

impl App {
    /// Draw the strategy chooser shell.
    pub(super) fn draw_strategies(&self, f: &mut Frame, area: Rect) {
        let dim = Style::default().add_modifier(Modifier::DIM);
        let selected = self
            .routing_strategy_index
            .min(ROUTING_STRATEGIES.len() - 1);
        let mut lines = Vec::new();
        for (index, option) in ROUTING_STRATEGIES.iter().enumerate() {
            let style = if index == selected {
                self.theme.selection()
            } else {
                Style::default()
            };
            lines.push(TLine::from(Span::styled(
                format!(
                    "{} {}",
                    if index == selected { "▸" } else { " " },
                    option.label
                ),
                style,
            )));
            lines.push(TLine::from(Span::styled(
                format!("  {}", option.description),
                dim,
            )));
        }
        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled(
            "↑↓/jk choose · Enter apply · automatic strategies need refreshed worker details",
            dim,
        )));
        f.render_widget(
            Paragraph::new(Text::from(lines)).block(self.panel("Strategies")),
            area,
        );
    }
}
