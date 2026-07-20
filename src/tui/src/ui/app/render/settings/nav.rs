//! The Settings tab's left-hand navigation: group headings over the flat,
//! selectable subpage list.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::super::super::types::{App, SETTINGS_GROUPS, SETTINGS_SUBPAGES};

impl App {
    /// Draw the grouped subpage nav.
    ///
    /// Headings come from [`SETTINGS_GROUPS`] and are interleaved into the flat
    /// [`SETTINGS_SUBPAGES`] list at their start indices, so `settings_index`
    /// stays a plain index into the selectable rows and never has to skip over
    /// non-selectable ones.
    pub(super) fn draw_settings_nav(&mut self, f: &mut Frame, area: Rect) {
        let block = self.panel("Settings");
        let inner = block.inner(area);
        f.render_widget(block, area);

        let dim = Style::default().add_modifier(Modifier::DIM);
        let mut lines: Vec<TLine> = Vec::new();
        for (i, name) in SETTINGS_SUBPAGES.iter().enumerate() {
            if let Some((heading, _)) = SETTINGS_GROUPS.iter().find(|(_, start)| *start == i) {
                lines.push(TLine::from(Span::styled(format!(" {heading}"), dim)));
            }
            // The selected row is highlighted only while the nav holds focus.
            // Once focus moves into the content pane it drops to a marker, so
            // there is never more than one "live" cursor on screen.
            let style = match (i == self.settings_index, self.settings_focused) {
                (true, false) => self.theme.selection(),
                (true, true) => Style::default().add_modifier(Modifier::BOLD),
                _ => Style::default(),
            };
            let marker = if i == self.settings_index && self.settings_focused {
                "▸"
            } else {
                " "
            };
            lines.push(TLine::from(Span::styled(
                format!(" {marker}{} {name} ", i + 1),
                style,
            )));
        }
        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled(
            if self.settings_focused {
                " Esc menu".to_string()
            } else {
                format!(" ↑↓ nav · ⏎ open · 1-{} jump", SETTINGS_SUBPAGES.len())
            },
            dim,
        )));
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }
}
