//! TokenMaxxxing leaderboard rendering: token burn, active days, rewards, and
//! previous daily winners.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::super::super::types::App;
use super::types::{LEADERBOARD, PREVIOUS_WINNERS};

impl App {
    /// Draw full standings with reward rules and recent daily winners.
    pub(super) fn draw_tokenmaxxing_leaderboard(&mut self, frame: &mut Frame, area: Rect) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(12),
                Constraint::Length(7),
                Constraint::Min(7),
            ])
            .split(area);
        for pane in rows.iter() {
            self.note_pane(*pane);
        }
        self.draw_token_burn_table(frame, rows[0]);
        self.draw_leaderboard_rewards(frame, rows[1]);
        self.draw_previous_winners(frame, rows[2]);
    }

    /// Draw GitHub users ranked by LLM token burn and days active on Medulla.
    pub(super) fn draw_token_burn_table(&self, frame: &mut Frame, area: Rect) {
        let narrow = area.width < 82;
        let mut lines = vec![if narrow {
            Line::from(Span::styled(
                format!("{:<4}{:<17}{:>9}{:>7}", "#", "GITHUB", "TOKENS", "DAYS"),
                Style::default().fg(self.theme.dim_border),
            ))
        } else {
            Line::from(Span::styled(
                format!(
                    "{:<5}{:<23}{:>14}{:>9}{:>10}   {}",
                    "#", "GITHUB USER", "TOKENS BURNED", "DAYS", "STREAK", "STATUS"
                ),
                Style::default().fg(self.theme.dim_border),
            ))
        }];

        for entry in LEADERBOARD.iter() {
            let row = if narrow {
                format!(
                    "{:<4}{:<17}{:>9}{:>7}",
                    entry.rank, entry.github, entry.tokens, entry.days
                )
            } else {
                format!(
                    "{:<5}{:<23}{:>14}{:>9}{:>10}   {}",
                    entry.rank, entry.github, entry.tokens, entry.days, entry.streak, entry.status
                )
            };
            let style = if entry.is_you {
                self.theme.selection()
            } else if matches!(entry.rank, "1" | "2" | "3") {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(row, style)));
        }

        let title = Line::from(vec![
            Span::styled(
                "Season leaderboard",
                Style::default()
                    .fg(self.theme.primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " · GitHub users ranked by LLM tokens burned + days on Medulla",
                Style::default().fg(self.theme.dim_border),
            ),
        ]);
        frame.render_widget(
            Paragraph::new(Text::from(lines)).block(self.panel("").title(title)),
            area,
        );
    }

    /// Explain the daily and season prizes attached to the standings.
    fn draw_leaderboard_rewards(&self, frame: &mut Frame, area: Rect) {
        let lines = vec![
            Line::from(vec![
                Span::styled("Daily token burner", Style::default().fg(Color::Yellow)),
                Span::raw("  $25 + 1,000 pts"),
            ]),
            Line::from(vec![
                Span::styled(
                    "Season token champion",
                    Style::default().fg(self.theme.primary),
                ),
                Span::raw("  $250 + TokenMaxxxer badge"),
            ]),
            Line::from(vec![
                Span::styled("Most days on Medulla", Style::default().fg(Color::Green)),
                Span::raw("  2,500 pts + Iron Streak badge"),
            ]),
            Line::from(Span::styled(
                "Dummy rewards for design review · eligibility and payout rules are not final",
                Style::default().fg(self.theme.dim_border),
            )),
        ];
        frame.render_widget(
            Paragraph::new(Text::from(lines)).block(self.panel("Rewards")),
            area,
        );
    }

    /// Draw recent daily winners so every reset leaves visible recognition.
    fn draw_previous_winners(&self, frame: &mut Frame, area: Rect) {
        let narrow = area.width < 82;
        let mut lines = vec![Line::from(Span::styled(
            if narrow {
                format!("{:<9}{:<18}{:>10}", "DAY", "GITHUB", "TOKENS")
            } else {
                format!(
                    "{:<12}{:<24}{:>14}   {}",
                    "DAY", "GITHUB WINNER", "TOKENS BURNED", "REWARD"
                )
            },
            Style::default().fg(self.theme.dim_border),
        ))];
        for winner in PREVIOUS_WINNERS.iter() {
            lines.push(Line::from(if narrow {
                format!(
                    "{:<9}{:<18}{:>10}",
                    winner.day, winner.github, winner.tokens
                )
            } else {
                format!(
                    "{:<12}{:<24}{:>14}   {}",
                    winner.day, winner.github, winner.tokens, winner.reward
                )
            }));
        }
        frame.render_widget(
            Paragraph::new(Text::from(lines)).block(self.panel("Previous daily winners")),
            area,
        );
    }
}
