//! The Help subpage: the keyboard reference, and the command list rendered from
//! the catalog itself so it cannot drift from what the composer accepts.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::ui::command::COMMANDS;
use crate::ui::harness_pane::FOCUS_CHORD_LABEL;

use super::super::super::types::App;

impl App {
    /// Draw the Help subpage.
    pub(super) fn draw_help(&mut self, f: &mut Frame, area: Rect) {
        let dim = Style::default().add_modifier(Modifier::DIM);
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let mut lines = vec![
            TLine::from("Tab / Shift-Tab switch views · Agents: type to compose, ↑↓ recall prompt history"),
            TLine::from(Span::styled(
                "In a multi-line draft ↑↓ walk the caret between rows; history recalls from the edge rows",
                dim,
            )),
            TLine::from("Enter sends · Shift-Enter inserts a newline (Option-Enter if Shift-Enter sends)"),
            TLine::from("PageUp / PageDown scrolls the transcript"),
            TLine::from("Overview: E prepared decisions"),
            TLine::from("Agents: Esc leaves the composer for the rail · ↑↓ walk it · Enter or type returns"),
            TLine::from("        ⌥↑↓ walks the rail without leaving the composer (needs Option-as-Meta)"),
            TLine::from("        ⌥X cancel task · ⌥A answer a question · Enter opens a template"),
            TLine::from("Agents: selecting an agent with an open question makes Enter answer it"),
            TLine::from("Routing: a add host · Enter/s select · e edit label · d/x remove"),
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
            TLine::from("Ctrl-N new thread · Ctrl-↑↓ switch threads · Ctrl-C quit"),
            TLine::from(" "),
            TLine::from(Span::styled("Sessions", bold)),
            TLine::from(format!("{FOCUS_CHORD_LABEL} type into the selected local session")),
            TLine::from(
                "Agents rail: Enter on + New agent declares one (harness type × workspace dir)",
            ),
            TLine::from(
                "Ctrl-T opens a session of the selected agent · elsewhere it starts a loose session",
            ),
            TLine::from(format!(
                "From an empty composer Esc focuses the rail · from a session {FOCUS_CHORD_LABEL} releases to it"
            )),
            TLine::from("On the rail ↑↓ select a running session · K then y kills it"),
            TLine::from(" "),
            TLine::from(Span::styled("Changes", bold)),
            TLine::from("Tab / Shift-Tab to the Changes view to inspect the Git diff since session start"),
            TLine::from("↑↓ select files · j/k move by line · [/] jump hunks · PageUp/PageDown move faster"),
            TLine::from(
                "c comments on a line or hunk · e edits it · C comments on or edits the file · r refreshes",
            ),
            TLine::from(" "),
            TLine::from(Span::styled("Copy", bold)),
            TLine::from("Ctrl-Y copies the whole chat · /copy last copies just the latest reply"),
            TLine::from(" "),
            TLine::from(Span::styled("Mouse", bold)),
            TLine::from("Click a tab to switch views · in Agents/Context click a row to select · wheel scrolls"),
            TLine::from("Ctrl-O / /mouse release the mouse to the terminal for native drag-select"),
            TLine::from(" "),
            TLine::from(Span::styled("Commands", bold)),
            TLine::from(Span::styled(
                "Type / in the composer to browse these · ↑↓ pick · Tab complete · Enter run",
                dim,
            )),
        ];
        // Rendered from the catalog rather than restated: a hand-kept list is
        // how a command ends up documented here and missing from the parser.
        let gutter = COMMANDS
            .iter()
            .map(|spec| spec.usage().chars().count())
            .max()
            .unwrap_or(0);
        for spec in COMMANDS {
            lines.push(TLine::from(vec![
                Span::styled(format!("{:<gutter$}", spec.usage()), Style::default()),
                Span::styled(format!("  {}", spec.description), dim),
            ]));
        }
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
