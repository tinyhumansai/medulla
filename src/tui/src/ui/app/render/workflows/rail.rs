//! The catalogue sidebar: every installed workflow, with the selected one's runs
//! indented beneath it.
//!
//! Styled like the Settings and Routing navs — a `▸` marker on the focused row,
//! digits to jump, and a hint line that changes with focus — because it does the
//! same job. The difference is that its entries come from the store rather than
//! a fixed list, so it cannot use [`crate::ui::multi_pane::draw_nav`] directly.

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
        let focused = self.wf.focus == WorkflowFocus::Sidebar;
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
            lines.push(self.rail_line(row, width, focused));
        }
        lines.push(TLine::from(Span::styled(
            clip(
                if focused {
                    "↑↓ nav · ⏎ open · 1-9 jump"
                } else {
                    "Esc list · c copilot"
                },
                width,
            ),
            Style::default().add_modifier(Modifier::DIM),
        )));
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }

    /// One sidebar row, styled for what it is and whether the cursor is on it.
    ///
    /// The marker distinguishes "this is the selection" from "the selection is
    /// here *and* the sidebar has the keyboard", which is the same two-state
    /// highlight the other navs use.
    fn rail_line(&self, row: &WorkflowRailRow, width: usize, focused: bool) -> TLine<'static> {
        let selected = self.workflow_rail_selected(row);
        let (text, degraded) = match row {
            WorkflowRailRow::Workflow { row, .. } => {
                (format!("{} · {}", row.label, row.detail), row.degraded)
            }
            // A run is indented under its workflow, led by enough of its id to
            // find it again with `medulla workflow get-run` and then by its
            // status, which is what anyone scanning the rail is looking for.
            WorkflowRailRow::Run { row, .. } => (
                format!(
                    "  {} {}",
                    medulla::ui::workflows::rows::short_run_id(&row.label),
                    row.detail
                ),
                row.degraded,
            ),
            WorkflowRailRow::Note(note) => (format!("  {note}"), true),
        };
        let mut style = Style::default();
        if degraded {
            style = style.add_modifier(Modifier::DIM);
        }
        if selected {
            style = if focused {
                self.theme.selection()
            } else {
                Style::default().add_modifier(Modifier::BOLD)
            };
        }
        let marker = if selected && focused { "▸" } else { " " };
        TLine::from(Span::styled(clip(&format!("{marker}{text}"), width), style))
    }
}

/// What the rail says when nothing is installed.
fn empty_lines() -> Vec<TLine<'static>> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    [
        "No workflows installed.",
        "",
        "Write one to",
        ".medulla/workflows/.",
    ]
    .into_iter()
    .map(|line| TLine::from(Span::styled(line, dim)))
    .collect()
}
