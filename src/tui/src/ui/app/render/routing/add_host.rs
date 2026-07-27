//! Guided entry point for connecting another host.
//!
//! The page is written as two steps because pairing is genuinely two steps, and
//! because each copy it asks for runs in the *easy* direction. Step one is
//! copied here — a local terminal, where copying is trivial — and pasted into
//! the remote shell. Step two is produced on the remote host, but the worker
//! hands it to this terminal's clipboard over OSC 52 rather than asking anyone
//! to select base58 out of an SSH scrollback (see
//! [`medulla::daemon::pairing`]).

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use medulla::daemon::pairing::REMOTE_JOIN_COMMAND;

use super::super::super::types::App;

impl App {
    /// Draw the Add Host instructions and action.
    pub(super) fn draw_add_host(&self, f: &mut Frame, area: Rect) {
        let dim = Style::default().add_modifier(Modifier::DIM);
        let step = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD);
        let lines = vec![
            TLine::from("Connect another machine to this orchestrator."),
            TLine::from(""),
            TLine::from(Span::styled("1 · On the machine you want to add", step)),
            TLine::from(""),
            TLine::from(Span::styled(
                format!("    {REMOTE_JOIN_COMMAND}"),
                Style::default().fg(Color::Green),
            )),
            TLine::from(""),
            TLine::from(Span::styled(
                "    Press c to copy that line, then paste it into an SSH session.",
                dim,
            )),
            TLine::from(""),
            TLine::from(Span::styled("2 · Back here", step)),
            TLine::from(""),
            TLine::from("    The worker prints its address and puts it on your clipboard,"),
            TLine::from("    even over SSH. Press a and paste it, with an optional label."),
            TLine::from(""),
            TLine::from(Span::styled(
                "    Example: 7Kx…9fQ Primary build machine",
                dim,
            )),
            TLine::from(""),
            TLine::from(Span::styled(
                "Skip the copy entirely by naming the worker: run it as",
                dim,
            )),
            TLine::from(Span::styled(
                "`medulla daemon --handle build-box` and add `@build-box` here.",
                dim,
            )),
            TLine::from(""),
            TLine::from(Span::styled(
                "c copy the install line · a add a host · Esc returns to the Routing menu",
                dim,
            )),
        ];
        f.render_widget(
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: true })
                .block(self.panel("Add Host")),
            area,
        );
    }
}
