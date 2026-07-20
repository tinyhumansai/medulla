//! The Help subpage: the keyboard and REPL command reference.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use super::super::super::types::App;

impl App {
    /// Draw the Help subpage.
    pub(super) fn draw_help(&mut self, f: &mut Frame, area: Rect) {
        let dim = Style::default().add_modifier(Modifier::DIM);
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let lines = vec![
            TLine::from("Tab / Shift-Tab switch views · Chat: type to compose, ↑↓ recall prompt history"),
            TLine::from(Span::styled(
                "In a multi-line draft ↑↓ walk the caret between rows; history recalls from the edge rows",
                dim,
            )),
            TLine::from("Chat pins to the latest reply; the composer is shown only on this view"),
            TLine::from("Enter sends · Shift-Enter inserts a newline (Option-Enter if Shift-Enter sends)"),
            TLine::from("PageUp / PageDown scrolls the Chat and Agents transcripts"),
            TLine::from("Agents: ↑↓ pick an agent · j / k scroll · X cancel task · A answer a question"),
            TLine::from("Workers: a add peer · Enter/s select · e edit label · d/x remove"),
            TLine::from("Memory: ↑↓ / j k browse directives, facets & hits · /memory <query> to search"),
            TLine::from(" "),
            TLine::from(Span::styled("Settings", bold)),
            TLine::from("↑↓ move between subpages · 1-8 jump straight to one"),
            TLine::from("Appearance: j / k pick a theme role · ←/→ or Enter cycle its color (saved live)"),
            TLine::from("Config: j / k pick a setting · ←/→ change · Enter toggle (saved to config.toml)"),
            TLine::from("Feedback: j / k browse · u/d vote · c comment · n feature · b bug · s sort · f filter"),
            TLine::from("Trace & Context (Debug): j / k page events and chunks"),
            TLine::from("Account: Enter twice to log out · Usage: r refresh"),
            TLine::from(" "),
            TLine::from("Ctrl-N new session · Ctrl-C quit (nav keys act only when the input is empty)"),
            TLine::from(" "),
            TLine::from(Span::styled("Copy", bold)),
            TLine::from("Ctrl-Y copies the whole chat · /copy last copies just the latest reply"),
            TLine::from(" "),
            TLine::from(Span::styled("Mouse", bold)),
            TLine::from("Click a tab to switch views · in Agents/Context click a row to select · wheel scrolls"),
            TLine::from("Ctrl-O / /mouse release the mouse to the terminal for native drag-select"),
            TLine::from(" "),
            TLine::from(Span::styled("Commands", bold)),
            TLine::from("/new · /fork [name] · /resume · /abort · /clear · /config · /copy [all|last]"),
            TLine::from("/usage · /settings · /theme · /memory [query] · /feedback · /mouse · /async [on|off]"),
            TLine::from("/help · /quit"),
        ];
        f.render_widget(
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: true })
                .block(self.panel("Keyboard & REPL help")),
            area,
        );
    }
}
