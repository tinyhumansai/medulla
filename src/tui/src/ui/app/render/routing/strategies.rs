//! Operator-selectable model and worker routing strategy surface.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::super::super::types::{App, ROUTING_STRATEGIES};

const LABELS: [&str; 4] = ["Manual", "Balanced", "CPU First", "Memory First"];
const DESCRIPTIONS: [&str; 4] = [
    "Keep the worker explicitly selected in List Workers.",
    "Blend logical CPU cores with currently available RAM.",
    "Choose the worker with the most logical CPU cores.",
    "Choose the worker with the most currently available RAM.",
];

impl App {
    /// Draw the strategy chooser shell.
    pub(super) fn draw_strategies(&self, f: &mut Frame, area: Rect) {
        let dim = Style::default().add_modifier(Modifier::DIM);
        let selected = self
            .routing_strategy_index
            .min(ROUTING_STRATEGIES.len() - 1);
        let mut lines = Vec::new();
        for (index, label) in LABELS.iter().enumerate() {
            let style = if index == selected {
                self.theme.selection()
            } else {
                Style::default()
            };
            lines.push(TLine::from(Span::styled(
                format!("{} {label}", if index == selected { "▸" } else { " " }),
                style,
            )));
            lines.push(TLine::from(Span::styled(
                format!("  {}", DESCRIPTIONS[index]),
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
