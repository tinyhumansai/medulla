//! The Account subpage: which backend this session is signed in to, and the
//! logout action.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use super::super::super::types::App;

impl App {
    /// Draw the Account subpage.
    pub(super) fn draw_account(&mut self, f: &mut Frame, area: Rect) {
        let dim = Style::default().add_modifier(Modifier::DIM);
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let backend = &self.loaded.config.backend;

        let mut lines = vec![
            TLine::from(Span::styled("Backend", bold)),
            TLine::from(format!(
                "host       {}",
                medulla::config::display_host(&backend.base_url)
            )),
            TLine::from(Span::styled(
                format!("url        {}", backend.base_url),
                dim,
            )),
        ];

        // Whether a token is present, and where it came from. Never the token.
        let store = self
            .medulla_home
            .as_ref()
            .map(|home| medulla::auth::CredentialStore::at_home(home));
        let stored = store.as_ref().and_then(|s| s.load());
        let env_token = std::env::var(&backend.token_env)
            .ok()
            .filter(|v| !v.trim().is_empty());
        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled("Credentials", bold)));
        match (&stored, &env_token) {
            (Some(_), _) => {
                lines.push(TLine::from(Span::styled(
                    "● signed in (stored credentials)",
                    Style::default().fg(Color::Green),
                )));
                if let Some(s) = &store {
                    lines.push(TLine::from(Span::styled(
                        format!("stored in  {}", s.path().display()),
                        dim,
                    )));
                }
            }
            (None, Some(_)) => lines.push(TLine::from(Span::styled(
                format!("● signed in via ${}", backend.token_env),
                Style::default().fg(Color::Green),
            ))),
            (None, None) => lines.push(TLine::from(Span::styled(
                "○ signed out · run `medulla login`",
                dim,
            ))),
        }
        if stored.is_some() && env_token.is_some() {
            lines.push(TLine::from(Span::styled(
                format!("${} is also set and takes precedence", backend.token_env),
                dim,
            )));
        }

        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled("Log out", bold)));
        if self.medulla_home.is_none() {
            lines.push(TLine::from(Span::styled(
                "unavailable · no Medulla home configured",
                dim,
            )));
        } else if self.logout_armed() {
            lines.push(TLine::from(Span::styled(
                "▸ Press Enter again to clear stored credentials",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(TLine::from(Span::styled(
                "Move to another setting to cancel.",
                dim,
            )));
        } else if stored.is_some() {
            lines.push(TLine::from("▸ Enter — log out"));
            lines.push(TLine::from(Span::styled(
                "Clears the stored credentials. This session keeps running until you quit.",
                dim,
            )));
        } else {
            lines.push(TLine::from(Span::styled("Nothing stored to clear.", dim)));
            lines.push(TLine::from(Span::styled(
                "▸ Enter — clear the credential file anyway",
                dim,
            )));
        }

        f.render_widget(
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: false })
                .block(self.panel("Account")),
            area,
        );
    }
}
