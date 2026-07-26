//! The directories the fleet can work in.
//!
//! A host answers "what capacity is there"; this answers "what is there to work
//! on", which is the question the orchestrator actually reasons about. What is
//! listed here for this device is exactly what reaches the orchestrator as
//! `capabilities.accessibleDirs`, so the page is a view of its context rather
//! than a separate setting that happens to resemble one.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::super::super::types::App;

impl App {
    /// Draw the workspace list with selection and the keys that change it.
    pub(super) fn draw_workspaces(&mut self, f: &mut Frame, area: Rect) {
        let rows = self.workspace_rows();
        let selected = self.workspace_index.min(rows.len().saturating_sub(1));
        self.workspace_index = selected;
        let block = self.panel(format!("Workspaces · {}", rows.len()));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let dim = Style::default().add_modifier(Modifier::DIM);

        let mut lines: Vec<TLine> = Vec::new();
        if rows.is_empty() {
            lines.push(TLine::from(
                "No workspaces declared — this device is not hosting.",
            ));
            lines.push(TLine::from(""));
            lines.push(TLine::from(Span::styled(
                "Press a to add a directory the orchestrator may work in.",
                dim,
            )));
        } else {
            // Two rows per workspace (path, then its context line), plus the
            // footer. Reserving the footer first keeps the action hints visible
            // however long the list gets.
            let visible = usize::from(inner.height)
                .saturating_sub(2)
                .checked_div(2)
                .unwrap_or(0)
                .max(1);
            let start = crate::ui::selection::viewport_start(selected, rows.len(), visible);
            for (index, row) in rows.iter().enumerate().skip(start).take(visible) {
                // The marker carries the one distinction that changes what a
                // delegated task does: work runs in the primary directory, the
                // others are context the orchestrator routes by.
                let marker = if row.primary { "●" } else { "·" };
                let mut style = if row.primary {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default()
                };
                if index == selected {
                    style = self.theme.selection();
                }
                lines.push(TLine::from(Span::styled(
                    format!("{marker} {} · {}", row.label, row.path),
                    style,
                )));

                let mut notes: Vec<String> = Vec::new();
                notes.push(match &row.host {
                    Some(host) => format!("on {host}"),
                    None => "this device".to_string(),
                });
                if row.primary {
                    notes.push("where tasks run".to_string());
                }
                if !row.exists {
                    notes.push("missing on disk".to_string());
                }
                if !row.editable {
                    notes.push("declared remotely".to_string());
                }
                if let Some(detail) = &row.detail {
                    notes.push(detail.clone());
                }
                lines.push(TLine::from(Span::styled(
                    format!("    {}", notes.join(" · ")),
                    if row.exists {
                        dim
                    } else {
                        Style::default().fg(Color::Red)
                    },
                )));
            }
        }
        lines.push(TLine::from(Span::styled(
            "a add a directory · d remove the selected one · Esc back to the menu",
            dim,
        )));
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }
}
