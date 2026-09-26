//! The Options tab: one group per status-line field, and the rows under it.
//!
//! A group is its field's name over the two or three rows that answer where it
//! sits, when it is drawn, and how it is spelled. Splitting the name off the
//! rows is what lets the rows be labelled with the question they ask rather
//! than with the field — "position" and "shown" mean the same thing in every
//! group, so the page reads as one form repeated rather than as seventeen
//! unrelated switches.
//!
//! What the field *is* — "the ●/✓/✕ dot: running, finished, or failed" — used to
//! sit under every heading. It is in the footer now, for the selected row only:
//! seven descriptions cost seven lines to answer a question an operator asks
//! about one field at a time, and they were what pushed the list past the fold.

use medulla::config::StatusLineConfig;
use ratatui::style::Style;
use ratatui::text::{Line as TLine, Span};

use crate::ui::app::status_line::STATUS_LINE_ROWS;
use crate::ui::app::types::App;

/// The column the question labels are padded to, so the controls line up.
///
/// Narrow deliberately: the settings nav takes twenty columns, which leaves an
/// eighty-column terminal well under sixty for this pane.
const LABEL_COLUMN: usize = 11;

/// The width a value is centred in between its stepper arrows. Sized to the
/// longest label any field offers ("when selected").
const VALUE_WIDTH: usize = 13;

impl App {
    /// Build the Options tab's lines, and the display line the cursor is on.
    ///
    /// The cursor's display line is recorded as the row is pushed rather than
    /// derived from the selection, because the headings and the blank line
    /// between groups sit between rows: a row's position on the page is not its
    /// position in the table.
    pub(super) fn status_line_fields(
        &self,
        selected: usize,
        cfg: &StatusLineConfig,
        dim: Style,
    ) -> (Vec<TLine<'static>>, usize) {
        let mut lines: Vec<TLine> = Vec::new();
        let mut selected_line = 0;

        for (index, row) in STATUS_LINE_ROWS.iter().enumerate() {
            if let Some(group) = row.group {
                if index > 0 {
                    lines.push(TLine::from(""));
                }
                lines.push(TLine::from(Span::styled(
                    group.title.to_string(),
                    Style::default().fg(self.theme.accent),
                )));
            }

            let style = if index == selected {
                self.theme.selection()
            } else {
                Style::default()
            };
            let marker = if index == selected { "▸ " } else { "  " };
            let (value, _) = row.field.value(cfg);
            if index == selected {
                selected_line = lines.len();
            }
            lines.push(TLine::from(vec![
                Span::styled(format!("{marker}{:<LABEL_COLUMN$}", row.label), style),
                // Stepper arrows, as on the Config subpage: they say the value
                // is one of several and that ←/→ walks them, which a bare value
                // does not.
                Span::styled(
                    format!("‹ {value:^VALUE_WIDTH$} ›"),
                    if index == selected { style } else { dim },
                ),
            ]));
        }

        (lines, selected_line)
    }
}
