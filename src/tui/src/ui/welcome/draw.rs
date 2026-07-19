//! The welcome screen's ratatui rendering: [`WelcomeScreen::draw`] paints the
//! centered panel for the current [`Step`](super::types), plus the meter and
//! layout helpers. State lives in [`super::types`]; the state machine in
//! [`super::state`].
//!
//! The consent step is the one screen here with a hard content requirement: it
//! must state exactly what leaves the machine, and that secrets are stripped
//! first. Everything else is presentation.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::Frame;

use super::types::{format_usd as usd, Step, WelcomeScreen};

/// Width of the reward meter drawn on the reveal.
const METER_WIDTH: usize = 24;

impl WelcomeScreen {
    /// Render the centered welcome panel.
    pub fn draw(&mut self, f: &mut Frame) {
        let area = centered_rect(70, 20, f.area());
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Cyan))
            .title(Span::styled(
                " welcome ",
                Style::default().add_modifier(Modifier::DIM),
            ));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(Span::styled(
            "MEDULLA",
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            format!("earn up to {} in free credits", usd(self.max_reward_usd)),
            Style::default().add_modifier(Modifier::DIM),
        )));
        lines.push(Line::from(""));

        match self.step {
            Step::Intro => self.draw_intro(&mut lines),
            Step::Scanning => self.draw_scanning(&mut lines),
            Step::Consent => self.draw_consent(&mut lines),
            Step::Uploading => self.draw_uploading(&mut lines),
            Step::Reveal => self.draw_reveal(&mut lines),
            Step::Empty => self.draw_empty(&mut lines),
        }

        if let Some(err) = &self.error {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("error: {err}"),
                Style::default().fg(Color::Red),
            )));
        }

        f.render_widget(
            Paragraph::new(lines)
                .alignment(Alignment::Left)
                .wrap(Wrap { trim: false }),
            inner,
        );
    }

    /// The pitch: what the deal is, before anything is read.
    fn draw_intro(&self, lines: &mut Vec<Line>) {
        lines.push(heading("How much of a power user are you?"));
        lines.push(Line::from(""));
        lines.push(Line::from(
            "Share your Claude Code and Codex history and we'll size up",
        ));
        lines.push(Line::from(
            "your coding-agent mileage — the more you've built, the more",
        ));
        lines.push(Line::from("credit you start with."));
        lines.push(Line::from(""));
        lines.push(bullet("we read your local session transcripts"));
        lines.push(bullet("secrets are stripped before anything is sent"));
        lines.push(bullet("you approve the exact upload first"));
        lines.push(Line::from(""));
        lines.push(hint("Enter to look · Esc to skip"));
    }

    /// Reading local files. Nothing has left the machine at this point.
    fn draw_scanning(&self, lines: &mut Vec<Line>) {
        lines.push(heading("Reading your history"));
        lines.push(Line::from(""));
        lines.push(Line::from(format!(
            "{} scanning local sessions…",
            self.spinner()
        )));
        lines.push(Line::from(""));
        lines.push(hint("nothing has been sent yet · Esc to skip"));
    }

    /// The consent gate: exactly what would be uploaded.
    fn draw_consent(&self, lines: &mut Vec<Line>) {
        lines.push(heading("Here's what we found"));
        lines.push(Line::from(""));
        for (agent, count) in &self.scan.per_agent {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {agent:<10}"),
                    Style::default().add_modifier(Modifier::DIM),
                ),
                Span::styled(
                    format!("{count} sessions"),
                    Style::default().fg(Color::White),
                ),
            ]));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::raw("Uploading "),
            Span::styled(
                format!("{} sessions", self.scan.session_count),
                Style::default()
                    .fg(Color::LightCyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" (~{})", bytes(self.scan.total_bytes))),
        ]));
        if self.scan.skipped_oversize > 0 {
            lines.push(hint(&format!(
                "{} oversized session(s) will be skipped",
                self.scan.skipped_oversize
            )));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "API keys, tokens, and passwords are removed before sending.",
            Style::default().fg(Color::Green),
        )));
        lines.push(Line::from(Span::styled(
            "Transcripts are encrypted at rest.",
            Style::default().fg(Color::Green),
        )));
        lines.push(Line::from(""));
        lines.push(hint("Enter to share and claim · Esc to skip"));
    }

    /// Upload progress, with the running redaction count as reassurance.
    fn draw_uploading(&self, lines: &mut Vec<Line>) {
        lines.push(heading("Sharing your history"));
        lines.push(Line::from(""));
        let ratio = if self.upload_total == 0 {
            0.0
        } else {
            self.uploaded as f64 / self.upload_total as f64
        };
        lines.push(Line::from(vec![
            Span::raw(format!("{} ", self.spinner())),
            Span::styled(meter(ratio), Style::default().fg(Color::LightCyan)),
            Span::raw(format!(" {}/{}", self.uploaded, self.upload_total)),
        ]));
        lines.push(Line::from(""));
        if self.redactions > 0 {
            lines.push(Line::from(Span::styled(
                format!("{} secret(s) scrubbed before sending", self.redactions),
                Style::default().fg(Color::Green),
            )));
        }
        lines.push(Line::from(""));
        lines.push(hint("hang tight…"));
    }

    /// The payoff: power level, amount, and how it was earned.
    fn draw_reveal(&self, lines: &mut Vec<Line>) {
        if let Some(tier) = &self.tier {
            lines.push(Line::from(vec![
                Span::styled(
                    "POWER LEVEL  ",
                    Style::default().add_modifier(Modifier::DIM),
                ),
                Span::styled(
                    tier.to_uppercase(),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
            lines.push(Line::from(""));
        }

        lines.push(Line::from(vec![
            Span::styled(
                usd(self.awarded_usd),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" of {} earned", usd(self.max_reward_usd))),
        ]));
        let ratio = if self.max_reward_usd > 0.0 {
            self.awarded_usd / self.max_reward_usd
        } else {
            0.0
        };
        lines.push(Line::from(Span::styled(
            meter(ratio),
            Style::default().fg(Color::Green),
        )));
        lines.push(Line::from(""));

        for (label, amount) in &self.breakdown {
            if *amount <= 0.0 {
                continue;
            }
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {label:<16}"),
                    Style::default().add_modifier(Modifier::DIM),
                ),
                Span::styled(usd(*amount), Style::default().fg(Color::White)),
            ]));
        }

        lines.push(Line::from(""));
        if self.already_claimed {
            lines.push(hint("already claimed — no new credit was added"));
        } else if self.awarded_usd > 0.0 {
            lines.push(Line::from(Span::styled(
                "Credit has been added to your balance.",
                Style::default().fg(Color::Green),
            )));
        } else {
            lines.push(Line::from(
                "No credit this time — come back once you've logged more sessions.",
            ));
        }
        lines.push(Line::from(""));
        lines.push(hint("Enter to continue"));
    }

    /// Nothing found locally — a dead end, stated plainly.
    fn draw_empty(&self, lines: &mut Vec<Line>) {
        lines.push(heading("No local history found"));
        lines.push(Line::from(""));
        lines.push(Line::from(
            "We couldn't find any Claude Code or Codex sessions on this",
        ));
        lines.push(Line::from(
            "machine, so there's nothing to score yet. Use an agent for a",
        ));
        lines.push(Line::from("while and this offer will be waiting."));
        lines.push(Line::from(""));
        lines.push(hint("Enter to continue"));
    }
}

/// A magenta step heading.
fn heading(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::default().fg(Color::Magenta),
    ))
}

/// A dim hint/footer row.
fn hint(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::default().add_modifier(Modifier::DIM),
    ))
}

/// A bulleted feature row.
fn bullet(text: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("  · ", Style::default().fg(Color::Cyan)),
        Span::raw(text.to_string()),
    ])
}

/// A `[████░░░░]` meter for `ratio`, clamped to 0..=1.
fn meter(ratio: f64) -> String {
    let ratio = ratio.clamp(0.0, 1.0);
    let filled = (ratio * METER_WIDTH as f64).round() as usize;
    let filled = filled.min(METER_WIDTH);
    format!(
        "[{}{}]",
        "█".repeat(filled),
        "░".repeat(METER_WIDTH - filled)
    )
}

/// Human-readable byte size, e.g. `820 KB` / `4.1 MB`.
fn bytes(value: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    let value = value as f64;
    if value >= MB {
        format!("{:.1} MB", value / MB)
    } else if value >= KB {
        format!("{:.0} KB", value / KB)
    } else {
        format!("{value:.0} B")
    }
}

/// A `w`×`h` rectangle centered in `area` (clamped to the area's size).
fn centered_rect(w: u16, h: u16, area: Rect) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((area.height.saturating_sub(h)) / 2),
            Constraint::Length(h),
            Constraint::Min(0),
        ])
        .split(area);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length((area.width.saturating_sub(w)) / 2),
            Constraint::Length(w),
            Constraint::Min(0),
        ])
        .split(rows[1]);
    cols[1]
}

#[cfg(test)]
mod draw_helper_tests {
    use super::super::types::format_usd as usd;
    use super::{bytes, meter, METER_WIDTH};

    #[test]
    fn meter_fills_proportionally_and_clamps() {
        assert_eq!(meter(0.0), format!("[{}]", "░".repeat(METER_WIDTH)));
        assert_eq!(meter(1.0), format!("[{}]", "█".repeat(METER_WIDTH)));
        assert_eq!(meter(2.0), format!("[{}]", "█".repeat(METER_WIDTH)));
        assert_eq!(meter(-1.0), format!("[{}]", "░".repeat(METER_WIDTH)));
        assert!(meter(0.5).contains('█'));
        assert!(meter(0.5).contains('░'));
    }

    #[test]
    fn usd_drops_cents_for_whole_dollars() {
        assert_eq!(usd(25.0), "$25");
        assert_eq!(usd(0.0), "$0");
        assert_eq!(usd(7.5), "$7.50");
    }

    #[test]
    fn bytes_scales_units() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(2048), "2 KB");
        assert_eq!(bytes(5 * 1024 * 1024), "5.0 MB");
    }
}
