//! The pinned detail footer: what the selected row does, every value it can
//! take, and where the answer is written.
//!
//! Pinning it is what keeps the explanation from moving out from under the
//! cursor as the operator walks a list taller than the pane, and the choice
//! list is why it is worth its height: a row shows one value, and a value alone
//! says nothing about what else is on offer or what `←/→` is about to do.

use ratatui::style::Style;
use ratatui::text::{Line as TLine, Span};

use crate::ui::app::appearance::APPEARANCE_ROWS;
use crate::ui::app::types::App;

impl App {
    /// Build the footer for the selected row, most specific first.
    ///
    /// Ordered so that truncating from the bottom on a short pane drops the
    /// least specific lines: the explanation and the choices outlive the key
    /// hints, which outlive the file the change lands in.
    pub(super) fn appearance_footer(&self, dim: Style, width: usize) -> Vec<TLine<'static>> {
        let index = self.appearance_index.min(APPEARANCE_ROWS - 1);
        let row = self.appearance_row();
        let value = self.appearance_value(index);

        let mut lines = vec![
            TLine::from(Span::styled("─".repeat(width), dim)),
            TLine::from(Span::styled(row.help, dim)),
        ];

        let mut spans: Vec<Span<'static>> = Vec::new();
        for (position, choice) in self.appearance_choices(index).into_iter().enumerate() {
            if position > 0 {
                spans.push(Span::styled(" · ", dim));
            }
            let style = if choice == value {
                self.theme.selection()
            } else {
                dim
            };
            spans.push(Span::styled(choice, style));
        }
        lines.push(TLine::from(spans));

        lines.push(TLine::from(Span::styled(
            if self.appearance_split {
                // Both halves are on screen, so there is nothing for `Tab` to
                // switch and no reason to advertise it.
                "↑↓ or j/k select · ←/→ or Enter change · applies live"
            } else {
                "↑↓ or j/k select · ←/→ or Enter change · Tab previews"
            },
            dim,
        )));
        lines.push(TLine::from(Span::styled(
            match &self.config_path {
                Some(path) => format!("saved to {}", path.display()),
                None => "changes apply live (no config path set)".into(),
            },
            dim,
        )));
        lines
    }
}
