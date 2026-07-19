//! The onboarding screen's ratatui rendering: [`OnboardingScreen::draw`] paints
//! the centered panel for the current [`Step`](super::types), plus the local
//! layout and summary-row helpers. State lives in [`super::types`]; the state
//! machine in [`super::state`].

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::Frame;

use super::types::{OnboardingScreen, Step};

impl OnboardingScreen {
    /// Render the centered onboarding panel.
    pub fn draw(&mut self, f: &mut Frame) {
        let area = centered_rect(66, 18, f.area());
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Cyan))
            .title(Span::styled(
                " worker setup ",
                Style::default().add_modifier(Modifier::DIM),
            ));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(Span::styled(
            "MEDULLA WORKER",
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            "first-run registration",
            Style::default().add_modifier(Modifier::DIM),
        )));
        lines.push(Line::from(""));

        match self.step {
            Step::Name => {
                lines.push(Line::from(Span::styled(
                    "Step 1/3 · name this worker",
                    Style::default().fg(Color::Magenta),
                )));
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::raw("name > "),
                    Span::styled(
                        self.name.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                ]));
                lines.push(Line::from(Span::styled(
                    "Enter to accept",
                    Style::default().add_modifier(Modifier::DIM),
                )));
            }
            Step::Connecting => {
                lines.push(Line::from(Span::styled(
                    "Step 2/3 · connection",
                    Style::default().fg(Color::Magenta),
                )));
                lines.push(Line::from(""));
                lines.push(Line::from(format!(
                    "{} setting up the tiny.place identity…",
                    self.spinner()
                )));
            }
            Step::Owner => {
                lines.push(Line::from(Span::styled(
                    "Step 2/3 · connection",
                    Style::default().fg(Color::Magenta),
                )));
                lines.push(Line::from(""));
                if let Some(address) = &self.address {
                    lines.push(Line::from(vec![
                        Span::styled("address ", Style::default().add_modifier(Modifier::DIM)),
                        Span::styled(address.clone(), Style::default().fg(Color::Green)),
                    ]));
                }
                if let Some(handle) = &self.handle {
                    lines.push(Line::from(vec![
                        Span::styled("handle  ", Style::default().add_modifier(Modifier::DIM)),
                        Span::styled(handle.clone(), Style::default().fg(Color::Cyan)),
                    ]));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "OpenHuman owner (@handle or address):",
                    Style::default().add_modifier(Modifier::DIM),
                )));
                lines.push(Line::from(vec![
                    Span::raw("owner > "),
                    Span::styled(
                        self.owner.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                ]));
                lines.push(Line::from(Span::styled(
                    "Enter to save · Esc to skip",
                    Style::default().add_modifier(Modifier::DIM),
                )));
            }
            Step::Confirm => {
                lines.push(Line::from(Span::styled(
                    "Step 3/3 · confirm",
                    Style::default().fg(Color::Magenta),
                )));
                lines.push(Line::from(""));
                lines.push(summary_line("name", &self.name));
                lines.push(summary_line(
                    "address",
                    self.address.as_deref().unwrap_or("(none)"),
                ));
                lines.push(summary_line(
                    "owner",
                    if self.owner.trim().is_empty() {
                        "(none — set later)"
                    } else {
                        self.owner.trim()
                    },
                ));
                lines.push(summary_line("endpoint", &self.endpoint));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Enter to finish · Esc to edit owner · q to abort",
                    Style::default().add_modifier(Modifier::DIM),
                )));
            }
        }

        if let Some(flash) = &self.flash {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                flash.clone(),
                Style::default().fg(Color::Green),
            )));
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
}

/// A dim `label` / white `value` summary row for the confirm panel.
fn summary_line<'a>(label: &'a str, value: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!("{label:<9}"),
            Style::default().add_modifier(Modifier::DIM),
        ),
        Span::styled(value.to_string(), Style::default().fg(Color::White)),
    ])
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
