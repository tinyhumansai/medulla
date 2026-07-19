//! Login-screen rendering: the centered ratatui panel
//! ([`LoginScreen::draw`]) plus the spinner frame and the display/layout
//! helpers it relies on.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::ui::app::SPINNER;

use super::types::{LoginScreen, Phase};

impl LoginScreen {
    fn spinner(&self) -> &'static str {
        SPINNER[self.frame % SPINNER.len()]
    }

    /// Render the centered login panel.
    pub fn draw(&mut self, f: &mut Frame) {
        let area = centered_rect(64, 17, f.area());
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Cyan))
            .title(Span::styled(
                " login ",
                Style::default().add_modifier(Modifier::DIM),
            ));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let mut lines: Vec<Line> = Vec::new();
        for row in crate::ui::LOGO {
            lines.push(Line::from(Span::styled(
                row,
                Style::default()
                    .fg(Color::LightCyan)
                    .add_modifier(Modifier::BOLD),
            )));
        }
        lines.push(Line::from(Span::styled(
            format!("backend {}", self.base_url),
            Style::default().add_modifier(Modifier::DIM),
        )));
        lines.push(Line::from(vec![
            Span::styled("provider ", Style::default().add_modifier(Modifier::DIM)),
            Span::styled(
                self.provider.as_str(),
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(""));

        match self.phase {
            Phase::Idle => {
                lines.push(Line::from(Span::styled(
                    "Enter/o  log in via browser",
                    Style::default(),
                )));
                lines.push(Line::from("←/→ or p  change provider"));
                lines.push(Line::from("t  paste a token"));
                lines.push(Line::from("m  continue offline (mock)"));
                lines.push(Line::from("q  quit"));
            }
            Phase::Starting => {
                lines.push(Line::from(format!("{} starting loopback…", self.spinner())));
            }
            Phase::Waiting => {
                if let Some(url) = &self.url {
                    lines.push(Line::from(Span::styled(
                        url.clone(),
                        Style::default().fg(Color::Blue),
                    )));
                    lines.push(Line::from(""));
                }
                let port = self.port.map(|p| p.to_string()).unwrap_or_default();
                lines.push(Line::from(format!(
                    "{} waiting for browser callback on 127.0.0.1:{port} …",
                    self.spinner()
                )));
                lines.push(Line::from(Span::styled(
                    "Esc  cancel",
                    Style::default().add_modifier(Modifier::DIM),
                )));
            }
            Phase::TokenEntry => {
                lines.push(Line::from(Span::styled(
                    "Paste a JWT or 64-hex login token, Enter to submit:",
                    Style::default().add_modifier(Modifier::DIM),
                )));
                let shown = token_display(&self.input, 56);
                lines.push(Line::from(vec![
                    Span::raw("> "),
                    Span::styled(shown, Style::default().add_modifier(Modifier::DIM)),
                ]));
                lines.push(Line::from(Span::styled(
                    "Esc  cancel",
                    Style::default().add_modifier(Modifier::DIM),
                )));
            }
            Phase::Verifying => {
                lines.push(Line::from(format!("{} verifying…", self.spinner())));
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
            lines.push(Line::from(Span::styled(
                "press Enter/o to retry",
                Style::default().add_modifier(Modifier::DIM),
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

/// Dim/truncate a token for display (no masking): show a leading window with an
/// ellipsis when it overflows `width`.
///
/// `pub(super)` so the sibling `tests` module can exercise it directly.
pub(super) fn token_display(token: &str, width: usize) -> String {
    if token.is_empty() {
        return String::new();
    }
    let count = token.chars().count();
    if count <= width {
        token.to_string()
    } else {
        let take = width.saturating_sub(1);
        let mut out: String = token.chars().take(take).collect();
        out.push('…');
        out
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
