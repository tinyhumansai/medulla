//! The scrolling upper region of the Status line page: the preview, then one
//! group of editable rows per status-line field.
//!
//! A group is a heading naming the field, a thin line saying what that field
//! puts on a harness row, and the two or three rows that answer where it sits,
//! when it is drawn, and how it is spelled. Splitting the name off the rows is
//! what lets the rows be labelled with the question they ask rather than with
//! the field — "position" and "shown" mean the same thing in every group, so the
//! page reads as one form repeated rather than as fifteen unrelated switches.

use medulla::config::StatusLineConfig;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span};

use crate::ui::app::render::sessions::RAIL_MAX_CONTENT;
use crate::ui::app::status_line::STATUS_LINE_ROWS;
use crate::ui::app::types::App;

/// The column the question labels are padded to, so the controls line up.
///
/// Narrow deliberately: the settings nav takes twenty columns, which leaves an
/// eighty-column terminal well under sixty for this pane, and the preview frame
/// already claims forty of them.
const LABEL_COLUMN: usize = 11;

/// The width a value is centred in between its stepper arrows. Sized to the
/// longest label any field offers ("when selected").
const VALUE_WIDTH: usize = 13;

impl App {
    /// Build the page's scrolling lines, and the index of the line the selected
    /// row was drawn on.
    ///
    /// The index is recorded as the row is pushed rather than derived from the
    /// selection, because headings, descriptions, and the blank line between
    /// groups all sit between rows: the row's position on the page is no longer
    /// its position in the table.
    pub(super) fn status_line_fields(
        &self,
        selected: usize,
        cfg: &StatusLineConfig,
        dim: Style,
    ) -> (Vec<TLine<'static>>, usize) {
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let mut lines: Vec<TLine> = Vec::new();
        let mut selected_line = 0;

        lines.push(TLine::from(vec![
            Span::styled("Preview", bold),
            Span::styled(
                format!("  · {RAIL_MAX_CONTENT} columns, as on the rail"),
                dim,
            ),
        ]));
        lines.extend(self.status_line_preview(dim));
        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled("Fields", bold)));

        for (index, row) in STATUS_LINE_ROWS.iter().enumerate() {
            if let Some(group) = row.group {
                lines.push(TLine::from(""));
                lines.push(TLine::from(Span::styled(
                    format!("  {}", group.title),
                    bold,
                )));
                lines.push(TLine::from(Span::styled(
                    format!("  {}", group.description),
                    dim,
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
