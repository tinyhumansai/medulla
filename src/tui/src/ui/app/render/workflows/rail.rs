//! The catalogue rail: every installed workflow, with the selected one's runs
//! indented beneath it.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::ui::util::clip;

use super::super::super::types::{App, WorkflowFocus};
use super::super::super::workflows::WorkflowRailRow;

impl App {
    /// Draw the catalogue and the selected workflow's run history.
    pub(super) fn draw_workflow_rail(&mut self, f: &mut Frame, area: Rect) {
        let focused = self.wf.focus == WorkflowFocus::Rail;
        let rows = self.workflow_rail_rows();
        let block = crate::ui::widgets::panel(
            &self.theme,
            format!("Workflows · {}", self.workflows.len()),
            focused,
        );
        let inner = block.inner(area);
        f.render_widget(block, area);
        if inner.height == 0 {
            return;
        }
        let width = inner.width as usize;

        if rows.is_empty() {
            f.render_widget(Paragraph::new(Text::from(empty_lines())), inner);
            return;
        }

        // The cursor's row drives the viewport, so scrolling past the bottom of
        // a long catalogue keeps the selection visible rather than the top.
        let cursor = rows
            .iter()
            .position(|row| self.workflow_rail_selected(row))
            .unwrap_or(0);
        let capacity = (inner.height as usize).saturating_sub(1).max(1);
        let start = crate::ui::selection::viewport_start(cursor, rows.len(), capacity);

        let mut lines: Vec<TLine> = Vec::new();
        for row in rows.iter().skip(start).take(capacity) {
            lines.push(self.rail_line(row, width));
        }
        lines.push(TLine::from(Span::styled(
            clip("↑↓ browse · ⏎ run · d dry-run · r refresh", width),
            Style::default().add_modifier(Modifier::DIM),
        )));
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }

    /// One rail row, styled for what it is and whether the cursor is on it.
    fn rail_line(&self, row: &WorkflowRailRow, width: usize) -> TLine<'static> {
        let selected = self.workflow_rail_selected(row);
        let (text, degraded) = match row {
            WorkflowRailRow::Workflow { row, .. } => {
                (format!("{} · {}", row.label, row.detail), row.degraded)
            }
            // A run is indented under its workflow and led by its status, which
            // is the part of a run anyone scanning the rail is looking for.
            WorkflowRailRow::Run { row, .. } => (format!("  {}", row.detail), row.degraded),
            WorkflowRailRow::Note(note) => (format!("  {note}"), true),
        };
        let mut style = Style::default();
        if degraded {
            style = style.add_modifier(Modifier::DIM);
        }
        if selected {
            style = self.theme.selection();
        }
        TLine::from(Span::styled(clip(&text, width), style))
    }
}

/// What the rail says when nothing is installed.
fn empty_lines() -> Vec<TLine<'static>> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    [
        "No workflows installed.",
        "",
        "The copilot beside the",
        "canvas can write one.",
    ]
    .into_iter()
    .map(|line| TLine::from(Span::styled(line, dim)))
    .collect()
}
