//! Subscription and API-key management surface.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::super::super::types::App;

impl App {
    /// Draw the credentials overview without exposing secret values.
    pub(super) fn draw_manage_keys(&self, f: &mut Frame, area: Rect) {
        let lines = vec![
            TLine::from("Provider subscriptions and API keys"),
            TLine::from(""),
            TLine::from(Span::styled(
                "No provider credentials have been added through Routing yet.",
                Style::default().add_modifier(Modifier::DIM),
            )),
        ];
        f.render_widget(
            Paragraph::new(Text::from(lines)).block(self.panel("Manage Keys")),
            area,
        );
    }
}
