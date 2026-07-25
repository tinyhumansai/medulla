//! Subscription and API-key management surface.

use ratatui::layout::Rect;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::super::super::types::App;

impl App {
    /// Draw the credentials overview without exposing secret values.
    pub(super) fn draw_manage_keys(&self, f: &mut Frame, area: Rect) {
        let lines = vec![
            TLine::from(Span::styled(
                "Provider subscriptions",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            credential_line(
                "Claude Code",
                self.credential_status.claude_subscription,
                "run `claude /login`",
            ),
            credential_line(
                "Codex",
                self.credential_status.codex_subscription,
                "run `codex login`",
            ),
            TLine::from(""),
            TLine::from(Span::styled(
                "API keys",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            credential_line(
                "Anthropic",
                self.credential_status.anthropic_api_key,
                "set $ANTHROPIC_API_KEY",
            ),
            credential_line(
                "OpenAI",
                self.credential_status.openai_api_key,
                "set $OPENAI_API_KEY",
            ),
            credential_line(
                "OpenRouter",
                self.credential_status.openrouter_api_key,
                "set $OPENROUTER_API_KEY",
            ),
            TLine::from(""),
            TLine::from(Span::styled(
                "Press r to refresh detected credentials.",
                Style::default().add_modifier(Modifier::DIM),
            )),
            TLine::from(Span::styled(
                "Secret values are never rendered. Subscription sessions remain owned by their provider CLIs.",
                Style::default().add_modifier(Modifier::DIM),
            )),
        ];
        f.render_widget(
            Paragraph::new(Text::from(lines)).block(self.panel("Manage Keys")),
            area,
        );
    }
}

/// Render one credential status without revealing its value.
fn credential_line<'a>(label: &str, present: bool, absent_hint: &str) -> TLine<'a> {
    let (glyph, status, style) = if present {
        (
            "●",
            "connected".to_string(),
            Style::default().fg(Color::Green),
        )
    } else {
        (
            "○",
            format!("not connected · {absent_hint}"),
            Style::default().add_modifier(Modifier::DIM),
        )
    };
    TLine::from(Span::styled(format!("{glyph} {label:<12} {status}"), style))
}

#[cfg(test)]
mod tests;
