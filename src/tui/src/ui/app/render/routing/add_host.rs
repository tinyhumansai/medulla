//! Guided entry point for connecting another host.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use super::super::super::types::App;

impl App {
    /// Draw the Add Host instructions and action.
    pub(super) fn draw_add_host(&self, f: &mut Frame, area: Rect) {
        let dim = Style::default().add_modifier(Modifier::DIM);
        let lines = vec![
            TLine::from("Connect a tiny.place host to this Medulla hub."),
            TLine::from(""),
            TLine::from("Enter the host's address or @handle, followed by an optional label."),
            TLine::from("Example: @build-box Primary build machine"),
            TLine::from(""),
            TLine::from(Span::styled(
                "Press Enter or a to add a host · Esc returns to the Routing menu",
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
