//! The Options tab: every Appearance row, grouped by the surface it changes.
//!
//! One row is one line — `label` then `value`, in fixed columns — because the
//! page's whole job is to let an operator find the setting they came for and
//! see what it is currently set to. Anything longer than that belongs in the
//! footer, which only has to explain the one row under the cursor.

use ratatui::style::Style;
use ratatui::text::{Line as TLine, Span};

use crate::ui::app::appearance::{AppearanceControl, APPEARANCE_ROWS, APPEARANCE_TABLE};
use crate::ui::app::types::App;

/// Width of the label column, wide enough for the longest row name.
const LABEL_WIDTH: usize = 20;

impl App {
    /// Build the Options tab's lines, and the display line the cursor is on.
    ///
    /// The cursor's display line is returned rather than recomputed because the
    /// group headings and blank lines mean it is not `appearance_index`, and
    /// scrolling to the wrong line is how a selected row ends up off-screen.
    pub(super) fn appearance_options(&self) -> (Vec<TLine<'static>>, usize) {
        let selected = self.appearance_index.min(APPEARANCE_ROWS - 1);
        let mut lines: Vec<TLine<'static>> = Vec::new();
        let mut selected_line = 0;

        for (index, row) in APPEARANCE_TABLE.iter().enumerate() {
            if let Some(group) = row.group {
                if index > 0 {
                    lines.push(TLine::from(""));
                }
                lines.push(TLine::from(Span::styled(
                    group.to_string(),
                    Style::default().fg(self.theme.accent),
                )));
            }
            if index == selected {
                selected_line = lines.len();
            }
            let style = if index == selected {
                self.theme.selection()
            } else {
                Style::default()
            };
            let marker = if index == selected { "▸ " } else { "  " };
            // The swatch leads, in a column of its own, so the label and value
            // columns stay straight down the page whether a row has one or not.
            // A colour is the one value a name cannot convey; the name stays
            // beside it because that is what the config file will say.
            let swatch = match row.control {
                // Patched onto the row's own style rather than built fresh, so
                // a selected row's highlight runs unbroken behind the swatch.
                AppearanceControl::Color(role) => {
                    Span::styled("███ ", style.fg(self.theme.role(role)))
                }
                _ => Span::styled("    ", style),
            };
            lines.push(TLine::from(vec![
                Span::styled(marker.to_string(), style),
                swatch,
                Span::styled(
                    format!(
                        "{:<LABEL_WIDTH$} {}",
                        row.label,
                        self.appearance_value(index)
                    ),
                    style,
                ),
            ]));
        }

        (lines, selected_line)
    }
}
