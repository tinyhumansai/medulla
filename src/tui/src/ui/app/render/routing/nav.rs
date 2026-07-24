//! Left-hand navigation for the Routing tab.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::super::super::types::{App, ROUTING_SUBPAGES};

impl App {
    /// Draw the Routing page selector and focus hints.
    pub(super) fn draw_routing_nav(&self, f: &mut Frame, area: Rect) {
        let block = self.panel("Routing");
        let inner = block.inner(area);
        f.render_widget(block, area);
        let dim = Style::default().add_modifier(Modifier::DIM);
        let mut lines = vec![TLine::from(Span::styled(" FLEET", dim))];
        for (index, page) in ROUTING_SUBPAGES.iter().enumerate() {
            let style = match (index == self.routing_index, self.routing_focused) {
                (true, false) => self.theme.selection(),
                (true, true) => Style::default().add_modifier(Modifier::BOLD),
                _ => Style::default(),
            };
            let marker = if index == self.routing_index && self.routing_focused {
                "▸"
            } else {
                " "
            };
            lines.push(TLine::from(Span::styled(
                format!(" {marker}{} {page} ", index + 1),
                style,
            )));
        }
        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled(
            if self.routing_focused {
                " Esc menu".to_string()
            } else {
                " ↑↓ nav · ⏎ open · 1-4 jump".to_string()
            },
            dim,
        )));
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }
}
