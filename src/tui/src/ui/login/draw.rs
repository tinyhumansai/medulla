//! Login-screen rendering: the centered ratatui panel
//! ([`LoginScreen::draw`]) plus the spinner frame and the display/layout
//! helpers it relies on.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::ui::app::SPINNER;

use super::types::{LoginScreen, Phase, MENU, MENU_ACTIONS_START};

impl LoginScreen {
    fn spinner(&self) -> &'static str {
        SPINNER[self.frame % SPINNER.len()]
    }

    /// Render the centered login panel.
    pub fn draw(&mut self, f: &mut Frame) {
        let area = centered_rect(64, 24, f.area());
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
        lines.push(Line::from(""));

        match self.phase {
            Phase::Idle => {
                // One selectable list: sign-in providers, then the fallbacks.
                // The highlighted row is inverted so the selection is legible
                // without relying on color alone.
                for (i, item) in MENU.iter().enumerate() {
                    if i == MENU_ACTIONS_START {
                        lines.push(Line::from(Span::styled(
                            "─".repeat(30),
                            Style::default().add_modifier(Modifier::DIM),
                        )));
                    }
                    let selected = i == self.menu_index;
                    let style = if selected {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    lines.push(Line::from(Span::styled(
                        format!("{} {:<32}", if selected { "▸" } else { " " }, item.label()),
                        style,
                    )));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "↑↓ choose · Enter select",
                    Style::default().add_modifier(Modifier::DIM),
                )));
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
                    "Paste an API key, JWT, or 64-hex login token — Enter to submit:",
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
                "press Enter to try again",
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
