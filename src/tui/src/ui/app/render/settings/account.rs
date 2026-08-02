//! The Account subpage: which backend this session is signed in to, who it is
//! signed in as, and the logout action.

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

        // Who is signed in, and where that came from. Never the token itself.
        let signed_in = self
            .account
            .as_ref()
            .is_some_and(|state| state.is_authenticated);
        let env_token = std::env::var(&backend.token_env)
            .ok()
            .filter(|v| !v.trim().is_empty());
        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled("Session", bold)));
        if signed_in {
            let who = self
                .account
                .as_ref()
                .and_then(|state| state.user_id.clone())
                .unwrap_or_else(|| "this device".to_string());
            lines.push(TLine::from(Span::styled(
                format!("● signed in as {who}"),
                Style::default().fg(Color::Green),
            )));
            lines.push(TLine::from(Span::styled(
                "held by the embedded OpenHuman core",
                dim,
            )));
        } else if env_token.is_some() {
            lines.push(TLine::from(Span::styled(
                format!("● backend token from ${}", backend.token_env),
                Style::default().fg(Color::Green),
            )));
        } else {
            lines.push(TLine::from(Span::styled(
                "○ signed out · run `medulla login`",
                dim,
            )));
        }
        if signed_in && env_token.is_some() {
            lines.push(TLine::from(Span::styled(
                format!("${} is also set and takes precedence", backend.token_env),
                dim,
            )));
        }

        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled("Log out", bold)));
        if self.logout_armed() {
            lines.push(TLine::from(Span::styled(
                "▸ Press Enter again to end the session",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(TLine::from(Span::styled(
                "Move to another setting to cancel.",
                dim,
            )));
        } else if signed_in {
            lines.push(TLine::from("▸ Enter — log out"));
            lines.push(TLine::from(Span::styled(
                "Ends the session and returns to the login screen.",
                dim,
            )));
        } else {
            // Still offered: a session the core holds but this screen could not
            // read is exactly the state a logout is for.
            lines.push(TLine::from(Span::styled("Not signed in.", dim)));
            lines.push(TLine::from(Span::styled(
                "▸ Enter — clear any stored session anyway",
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
