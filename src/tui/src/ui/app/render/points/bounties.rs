//! TokenMaxxxing bounty rendering: progress, daily cards, and payout rules.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use super::super::super::types::App;
use super::types::DAILY_BOUNTIES;

impl App {
    /// Draw bounty progress, challenge cards, and the daily reward rules.
    pub(super) fn draw_tokenmaxxing_bounties(&mut self, frame: &mut Frame, area: Rect) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(5),
                Constraint::Length(if area.width >= 90 { 7 } else { 5 }),
                Constraint::Min(6),
            ])
            .split(area);
        for pane in rows.iter() {
            self.note_pane(*pane);
        }
        self.draw_bounty_progress(frame, rows[0]);
        self.draw_daily_bounties(frame, rows[1]);
        self.draw_bounty_rules(frame, rows[2]);
    }

    /// Draw today's claimed and still-available bounty points.
    fn draw_bounty_progress(&self, frame: &mut Frame, area: Rect) {
        let bar_width = area.width.saturating_sub(40).clamp(8, 32) as usize;
        let filled = bar_width.saturating_mul(120) / 400;
        let bar = format!(
            "{}{}",
            "━".repeat(filled),
            "─".repeat(bar_width.saturating_sub(filled))
        );
        let body = Text::from(vec![
            Line::from(vec![
                Span::styled(
                    "120 pts claimed",
                    Style::default()
                        .fg(self.theme.primary)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "  ·  280 pts still available today",
                    Style::default().fg(self.theme.dim_border),
                ),
            ]),
            Line::from(vec![
                Span::styled(bar, Style::default().fg(self.theme.accent)),
                Span::styled("  1 of 3 complete", Style::default().fg(Color::Green)),
            ]),
        ]);
        frame.render_widget(
            Paragraph::new(body).block(self.panel("Today's bounty run")),
            area,
        );
    }

    /// Draw three daily earning opportunities, collapsing to a compact list on
    /// terminals too narrow to give each bounty its own card.
    fn draw_daily_bounties(&self, frame: &mut Frame, area: Rect) {
        let title = Line::from(vec![
            Span::styled(
                "Daily bounties",
                Style::default()
                    .fg(self.theme.primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " · 1/3 claimed · refreshes in 08h 42m",
                Style::default().fg(self.theme.dim_border),
            ),
        ]);
        let block = self.panel("").title(title);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if area.width < 90 {
            let lines = DAILY_BOUNTIES
                .iter()
                .map(|bounty| {
                    let marker = if bounty.progress == bounty.target {
                        "✓"
                    } else {
                        "○"
                    };
                    Line::from(vec![
                        Span::styled(
                            format!("{marker} {:<22}", bounty.title),
                            Style::default().fg(if bounty.progress == bounty.target {
                                Color::Green
                            } else {
                                self.theme.primary
                            }),
                        ),
                        Span::styled(
                            format!(" {}/{}  {}", bounty.progress, bounty.target, bounty.reward),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                    ])
                })
                .collect::<Vec<_>>();
            frame.render_widget(Paragraph::new(Text::from(lines)), inner);
            return;
        }

        let cards = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Ratio(1, 3); 3])
            .split(inner);
        for (index, bounty) in DAILY_BOUNTIES.iter().enumerate() {
            let complete = bounty.progress == bounty.target;
            let marker = if complete {
                "✓ COMPLETE"
            } else {
                "○ IN PROGRESS"
            };
            let color = if complete {
                Color::Green
            } else {
                self.theme.primary
            };
            let card = Block::default()
                .borders(Borders::RIGHT)
                .border_style(Style::default().fg(self.theme.dim_border));
            let body = Text::from(vec![
                Line::from(Span::styled(
                    marker,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    bounty.title,
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    bounty.detail,
                    Style::default().fg(self.theme.dim_border),
                )),
                Line::from(vec![
                    Span::styled(bounty.reward, Style::default().fg(self.theme.accent)),
                    Span::raw(format!("  {}/{}", bounty.progress, bounty.target)),
                ]),
            ]);
            frame.render_widget(Paragraph::new(body).block(card), cards[index]);
        }
    }

    /// Explain the reset and payout rules beneath the dummy bounty cards.
    fn draw_bounty_rules(&self, frame: &mut Frame, area: Rect) {
        let lines = vec![
            Line::from("✓ Finish before 00:00 UTC to claim the reward"),
            Line::from("✓ Each bounty pays once per daily reset"),
            Line::from("✓ Rewards increase both your season total and daily score"),
            Line::from(Span::styled(
                "Dummy rules for design review · no rewards are persisted yet",
                Style::default().fg(self.theme.dim_border),
            )),
        ];
        frame.render_widget(
            Paragraph::new(Text::from(lines)).block(self.panel("How daily bounties work")),
            area,
        );
    }
}
