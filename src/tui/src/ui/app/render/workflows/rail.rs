//! The catalogue sidebar: every installed workflow, with the selected one's runs
//! indented beneath it.
//!
//! Drawn by [`crate::ui::multi_pane::draw_rows`], the same renderer behind the
//! Settings and Routing navs, so the `▸` marker, the selection styling, the
//! viewport, and the hint line are one implementation rather than three that
//! agree only by inspection. What this module owns is the part that is genuinely
//! specific: turning a workflow, and the runs nested under it, into rows.

use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::Frame;

use crate::ui::multi_pane::{self, NavRow};

use super::super::super::types::{App, WorkflowFocus};
use super::super::super::workflows::{WorkflowRailRow, NEW_LABEL};

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
                        WorkflowRailRow::Workflow { .. } | WorkflowRailRow::New => 0,
                        _ => RUN_INDENT,
                    },
                    selected: self.workflow_rail_selected(row),
                    dim: match row {
                        WorkflowRailRow::Workflow { row, .. }
                        | WorkflowRailRow::Run { row, .. } => row.degraded,
                        WorkflowRailRow::Hint(_) => true,
                        // Never dim: it is the one row that is an offer rather
                        // than a record, and on an empty machine it is the only
                        // thing there is to do.
                        WorkflowRailRow::New => false,
                    },
                    color: match row {
                        WorkflowRailRow::Run { status, .. } => Some(run_color(*status)),
                        _ => None,
                    },
                    selectable: !matches!(row, WorkflowRailRow::Hint(_)),
                })
                .collect();

        let selected = rows.iter().find(|row| self.workflow_rail_selected(row));
        let hint = rail_hint(focused, selected);
        multi_pane::draw_rows(f, area, block, &self.theme, &nav_rows, focused, hint, 0);
    }
}

/// Return shortcuts that are valid for the row currently holding the cursor.
pub(super) fn rail_hint(focused: bool, selected: Option<&WorkflowRailRow>) -> &'static str {
    if !focused {
        return "Esc list · c copilot";
    }
    if matches!(selected, Some(WorkflowRailRow::Workflow { .. })) {
        "↑↓ nav · ⏎ open · Del delete · 1-9 jump"
    } else {
        "↑↓ nav · ⏎ open · 1-9 jump"
    }
}

/// The text of one rail row, without marker, digit, or indent.
pub(in crate::ui::app) fn rail_label(row: &WorkflowRailRow) -> String {
    match row {
        WorkflowRailRow::Workflow { row, .. } => row.label.clone(),
        // Led by enough of the run's id to find it again with
        // `medulla workflow get-run`, then by its status — which is what anyone
        // scanning the rail is actually looking for.
        WorkflowRailRow::Run { row, status, .. } => format!(
            "{} {} {}",
            run_glyph(*status),
            medulla::ui::workflows::rows::short_run_id(&row.label),
            row.detail
        ),
        WorkflowRailRow::Hint(hint) => hint.clone(),
        WorkflowRailRow::New => NEW_LABEL.to_string(),
    }
}

/// Compact state marker shown before a run id.
pub(super) fn run_glyph(status: medulla::workflows::RunStatus) -> &'static str {
    use medulla::workflows::RunStatus;
    match status {
        RunStatus::Succeeded => "✓",
        RunStatus::Running | RunStatus::PendingApproval => "●",
        RunStatus::Failed | RunStatus::Cancelled | RunStatus::Interrupted => "✗",
    }
}

/// Semantic run colour requested by the workflow history surface.
pub(super) fn run_color(status: medulla::workflows::RunStatus) -> Color {
    super::super::color(medulla::ui::workflows::status_color(status))
}
