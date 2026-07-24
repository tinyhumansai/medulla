//! Subscription and API-key management surface.

use ratatui::layout::Rect;
use std::path::PathBuf;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::super::super::types::App;

impl App {
    /// Draw the credentials overview without exposing secret values.
    pub(super) fn draw_manage_keys(&self, f: &mut Frame, area: Rect) {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let claude_subscription = home
            .as_ref()
            .is_some_and(|path| path.join(".claude/.credentials.json").is_file())
            || env_present("CLAUDE_CODE_OAUTH_TOKEN");
        let codex_subscription = home
            .as_ref()
            .is_some_and(|path| path.join(".codex/auth.json").is_file());
        let lines = vec![
            TLine::from(Span::styled(
                "Provider subscriptions",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            credential_line("Claude Code", claude_subscription, "run `claude /login`"),
            credential_line("Codex", codex_subscription, "run `codex login`"),
            TLine::from(""),
            TLine::from(Span::styled(
                "API keys",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            credential_line(
                "Anthropic",
                env_present("ANTHROPIC_API_KEY"),
                "set $ANTHROPIC_API_KEY",
            ),
            credential_line(
                "OpenAI",
                env_present("OPENAI_API_KEY"),
                "set $OPENAI_API_KEY",
            ),
            credential_line(
                "OpenRouter",
                env_present("OPENROUTER_API_KEY"),
                "set $OPENROUTER_API_KEY",
            ),
            TLine::from(""),
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

/// Whether a credential environment variable is present and non-blank.
fn env_present(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| !value.trim().is_empty())
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
