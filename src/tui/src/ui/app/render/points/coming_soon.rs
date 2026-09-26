//! Production placeholder for TokenMaxxxing before rewards launch.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use super::super::super::types::App;

impl App {
    /// Hide all prototype data behind a production-safe launch message.
    pub(super) fn draw_tokenmaxxxing_coming_soon(&mut self, frame: &mut Frame, area: Rect) {
        let card = crate::ui::layout::centered_percent(area, 68, 52);
        self.note_pane(card);
        let body = Text::from(vec![
            Line::from(""),
            Line::from(Span::styled(
                "Burn tokens. Build streaks. Get recognized.",
                Style::default()
                    .fg(self.theme.primary)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("TokenMaxxxing will celebrate GitHub builders who put Medulla to work."),
            Line::from("Compete on LLM token burn, active days, and daily bounties."),
            Line::from(""),
            Line::from(Span::styled(
                "Rewards, rankings, and claims are not live yet.",
                Style::default().fg(self.theme.dim_border),
            )),
        ]);
        frame.render_widget(
            Paragraph::new(body)
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true })
                .block(self.panel("TokenMaxxxing · Coming soon")),
            card,
        );
    }
}
