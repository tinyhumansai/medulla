//! The catalogue sidebar: every installed workflow, with the selected one's runs
//! indented beneath it.
//!
//! Drawn by [`crate::ui::multi_pane::draw_rows`], the same renderer behind the
//! Settings and Routing navs, so the `▸` marker, the selection styling, the
//! viewport, and the hint line are one implementation rather than three that
//! agree only by inspection. What this module owns is the part that is genuinely
//! specific: turning a workflow, and the runs nested under it, into rows.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::ui::multi_pane::{self, NavRow};

use super::super::super::types::{App, WorkflowFocus};
use super::super::super::workflows::WorkflowRailRow;

/// How far a run sits under the workflow it belongs to.
const RUN_INDENT: usize = 2;

impl App {
    /// Draw the catalogue and the selected workflow's run history.
    pub(super) fn draw_workflow_rail(&mut self, f: &mut Frame, area: Rect) {
        let focused = self.wf.focus == WorkflowFocus::Sidebar;
        let block = crate::ui::widgets::panel(
            &self.theme,
            format!("Workflows · {}", self.workflow_summaries().len()),
            focused,
        );

        let rows = self.workflow_rail_rows();
        if rows.is_empty() {
            let inner = block.inner(area);
            f.render_widget(block, area);
            f.render_widget(Paragraph::new(Text::from(empty_lines())), inner);
            return;
        }

        // Labels are formatted per row and live nowhere on `App`, so they are
        // built as owned strings first and the borrowing `NavRow`s point into
        // them.
        let labels: Vec<String> = rows.iter().map(rail_label).collect();
        let nav_rows: Vec<NavRow> =
            rows.iter()
                .zip(labels.iter())
                .map(|(row, label)| NavRow {
                    label,
                    jump: match row {
                        // Only workflows are digit-jumpable. A run has no number an
                        // operator could have read off the screen.
                        WorkflowRailRow::Workflow { index, .. } => Some(index + 1),
                        _ => None,
                    },
                    indent: match row {
                        WorkflowRailRow::Workflow { .. } => 0,
                        _ => RUN_INDENT,
                    },
                    selected: self.workflow_rail_selected(row),
                    dim: match row {
                        WorkflowRailRow::Workflow { row, .. }
                        | WorkflowRailRow::Run { row, .. } => row.degraded,
                        WorkflowRailRow::Note(_) => true,
                    },
                    selectable: !matches!(row, WorkflowRailRow::Note(_)),
                })
                .collect();

        let hint = if focused {
            "↑↓ nav · ⏎ open · 1-9 jump"
        } else {
            "Esc list · c copilot"
        };
        multi_pane::draw_rows(f, area, block, &self.theme, &nav_rows, focused, hint, 0);
    }
}

/// The text of one rail row, without marker, digit, or indent.
fn rail_label(row: &WorkflowRailRow) -> String {
    match row {
        WorkflowRailRow::Workflow { row, .. } => format!("{} · {}", row.label, row.detail),
        // Led by enough of the run's id to find it again with
        // `medulla workflow get-run`, then by its status — which is what anyone
        // scanning the rail is actually looking for.
        WorkflowRailRow::Run { row, .. } => format!(
            "{} {}",
            medulla::ui::workflows::rows::short_run_id(&row.label),
            row.detail
        ),
        WorkflowRailRow::Note(note) => (*note).to_string(),
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
