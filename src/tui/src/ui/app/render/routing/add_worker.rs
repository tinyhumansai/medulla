//! Guided entry point for connecting another worker.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use super::super::super::types::App;

impl App {
    /// Draw the Add Worker instructions and action.
    pub(super) fn draw_add_worker(&self, f: &mut Frame, area: Rect) {
        let dim = Style::default().add_modifier(Modifier::DIM);
        let lines = vec![
            TLine::from("Connect a tiny.place worker to this Medulla hub."),
            TLine::from(""),
            TLine::from("Enter the worker's address or @handle, followed by an optional label."),
            TLine::from("Example: @build-box Primary build worker"),
            TLine::from(""),
            TLine::from(Span::styled(
                "Press Enter or a to add a worker · Esc returns to the Routing menu",
                dim,
            )),
        ];
        f.render_widget(
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: true })
                .block(self.panel("Add Worker")),
            area,
        );
    }
}
