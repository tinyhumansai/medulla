//! The Help subpage: the keyboard reference, and the command list rendered from
//! the catalog itself so it cannot drift from what the composer accepts.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::ui::harness_pane::FOCUS_CHORD_LABEL;

use super::super::super::types::App;

impl App {
    /// Draw the Help subpage.
    pub(super) fn draw_help(&mut self, f: &mut Frame, area: Rect) {
        let dim = Style::default().add_modifier(Modifier::DIM);
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let lines = vec![
            TLine::from("Tab / Shift-Tab switch views · Ctrl-C quit"),
            TLine::from("Overview: E prepared decisions"),
            TLine::from("Routing: a add host · Enter/s select · e edit label · d/x remove"),
            TLine::from(" "),
            TLine::from(Span::styled("Sessions", bold)),
            TLine::from("Ctrl-T opens a session: pick a harness type, then a workspace directory"),
            TLine::from("↑↓ walk the rail · Enter on the + row opens the picker"),
            TLine::from(format!(
                "{FOCUS_CHORD_LABEL} type into the selected local session"
            )),
            TLine::from("d shows what the session has changed · K then y kills it · k closes it"),
            TLine::from("⌥X cancels a dispatched task · ⌥A answers its open question"),
            TLine::from(" "),
            TLine::from(Span::styled("Subconscious", bold)),
            TLine::from(Span::styled(
                "Intake filtering, learnings, and human approvals will live here. Nothing is wired yet.",
                dim,
            )),
            TLine::from(" "),
            TLine::from(Span::styled("Settings", bold)),
            TLine::from("↑↓ move between subpages · 1-9 jump straight to one"),
            TLine::from("Appearance: j / k pick an option · ←/→ or Enter change it (saved live)"),
            TLine::from(
                "Status line: j / k pick a session-row field · ←/→ or Enter cycle it (live preview)",
            ),
            TLine::from("Config: j / k pick a setting · ←/→ change · Enter toggle (saved to config.toml)"),
            TLine::from("Feedback: j / k browse · u/d vote · c comment · n feature · b bug · s sort · f filter"),
            TLine::from("Trace & Context (Debug): j / k page events and chunks"),
            TLine::from("Account: Enter twice to log out · Usage: r refresh"),
            TLine::from(" "),
            TLine::from(Span::styled("Changes", bold)),
            TLine::from("On a session row, press d to inspect the Git diff since that session launched"),
            TLine::from("↑↓ select files · j/k move by line · [/] jump hunks · PageUp/PageDown move faster"),
            TLine::from(
                "c comments on a line or hunk · e edits it · C comments on or edits the file · r refreshes",
            ),
            TLine::from("d or Esc puts the harness terminal back"),
            TLine::from(" "),
            TLine::from(Span::styled("Mouse", bold)),
            TLine::from("Click a tab to switch views · click a rail row to select it · wheel scrolls"),
            TLine::from("Ctrl-O releases the mouse to the terminal for native drag-select"),
        ];
        // Clamped here rather than at the key press: how far this page can
        // scroll depends on the terminal it is being drawn into, which the key
        // handler does not know.
        let block = self.panel("Keyboard & REPL help");
        let inner = block.inner(area);
        let paragraph = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: true });
        // Logical lines are not screen rows once wrapping is enabled. Ask the
        // widget's own line breaker for its rendered height so narrow terminals
        // can scroll through every wrapped description to the final command.
        let rendered = paragraph.line_count(inner.width);
        let max_scroll = rendered.saturating_sub(inner.height as usize) as u16;
        self.help_scroll = self.help_scroll.min(max_scroll);
        f.render_widget(paragraph.scroll((self.help_scroll, 0)).block(block), area);
    }
}
