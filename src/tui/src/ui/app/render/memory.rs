//! The Memory tab's placeholder page.
//!
//! The persona-memory layer is out of the build for now — its engine dependency
//! went with it — so the tab states that plainly rather than being removed. A
//! tab that vanishes reads as a feature that was cancelled; one that says
//! "Coming soon" reads as the thing it is, and keeps the place it will return
//! to.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::super::types::App;

impl App {
    /// Draw the Memory tab's "coming soon" notice.
    pub(super) fn draw_memory(&mut self, frame: &mut Frame, area: Rect) {
        self.note_pane(area);
        let block = self.panel("Memory");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.height == 0 || inner.width == 0 {
            return;
        }
        let dim = Style::default().add_modifier(Modifier::DIM);
        let lines = vec![
            TLine::from(""),
            TLine::from(Span::styled(
                "  Coming soon",
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD),
            )),
            TLine::from(""),
            TLine::from(Span::styled(
                "  Persona memory — what the orchestrator has learned about how",
                dim,
            )),
            TLine::from(Span::styled("  you work — is not in this build yet.", dim)),
        ];
        frame.render_widget(Paragraph::new(Text::from(lines)), inner);
    }
}
