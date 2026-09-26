//! TokenMaxxxing overview rendering: personal season progress plus standings.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::super::super::types::App;

impl App {
    /// Draw season progress and the token-burn leaderboard preview.
    pub(super) fn draw_tokenmaxxing_overview(&mut self, frame: &mut Frame, area: Rect) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(6), Constraint::Min(9)])
            .split(area);
        for pane in rows.iter() {
            self.note_pane(*pane);
        }
        self.draw_points_summary(frame, rows[0]);
        self.draw_token_burn_table(frame, rows[1]);
    }

    /// Draw the current user's season position and next-level progress.
    fn draw_points_summary(&self, frame: &mut Frame, area: Rect) {
        let progress_width = area.width.saturating_sub(39).clamp(8, 34) as usize;
        let filled = progress_width.saturating_mul(2480) / 3000;
        let bar = format!(
            "{}{}",
            "━".repeat(filled),
            "─".repeat(progress_width.saturating_sub(filled))
        );
        let body = Text::from(vec![
            Line::from(vec![
                Span::styled(
                    "2,480 pts",
                    Style::default()
                        .fg(self.theme.primary)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "  ·  9.8M tokens burned  ·  12 days on Medulla",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled(bar, Style::default().fg(self.theme.accent)),
                Span::styled(
                    "  520 pts to Level 8",
                    Style::default().fg(self.theme.dim_border),
                ),
            ]),
            Line::from(Span::styled(
                "7 day streak  🔥  ·  #12 this season  ·  top 8%",
                Style::default().fg(Color::Yellow),
            )),
        ]);
        let title = Line::from(vec![
            Span::styled(
                "TokenMaxxxing",
                Style::default()
                    .fg(self.theme.primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " · Season 01 · 18 days left",
                Style::default().fg(self.theme.dim_border),
            ),
        ]);
        frame.render_widget(
            Paragraph::new(body).block(self.panel("").title(title)),
            area,
        );
    }
}
