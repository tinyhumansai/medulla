//! The Workflows page: the multi-step plans installed on this machine.
//!
//! A workflow is what an agent template is not: a template says what kind of
//! agent may be stood up, a workflow says what a *sequence* of them should do.
//! So it gets its own page beside the catalog rather than a branch of the fleet
//! tree — it describes work, not capacity.
//!
//! The list is what you scan and the run history is what you check afterwards,
//! so the selected workflow's recent runs sit under the list rather than behind
//! a keypress: "did last night's sweep pass" is the question this page is opened
//! to answer.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use medulla::ui::workflows::{run_rows, workflow_rows};

use crate::ui::util::clip;

use super::super::super::types::App;

/// How many recent runs to show under the list. Enough to see a pattern,
/// few enough to leave the catalog the bulk of the page.
const RECENT_RUNS: usize = 5;

impl App {
    /// Draw the installed workflows and the selected one's recent runs.
    pub(super) fn draw_workflows(&mut self, f: &mut Frame, area: Rect) {
        let rows = workflow_rows(self.workflow_summaries());
        let selected = self.workflow_index.min(rows.len().saturating_sub(1));
        self.workflow_index = selected;

        let block = self.panel(format!("Workflows · {}", rows.len()));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let mut lines: Vec<TLine> = Vec::new();
        if rows.is_empty() {
            // An empty store is a normal state, not a broken one — most machines
            // have no workflows until someone writes one.
            lines.push(TLine::from(Span::styled(
                "No workflows installed.",
                Style::default().add_modifier(Modifier::DIM),
            )));
            lines.push(TLine::from(""));
            lines.push(TLine::from(Span::styled(
                "Write one to .medulla/workflows/<id>.json, or have a coding agent",
                Style::default().add_modifier(Modifier::DIM),
            )));
            lines.push(TLine::from(Span::styled(
                "build one: medulla workflow catalog explains every node kind.",
                Style::default().add_modifier(Modifier::DIM),
            )));
            lines.push(TLine::from(""));
            lines.push(TLine::from(Span::styled(
                "r re-reads the store",
                Style::default().add_modifier(Modifier::DIM),
            )));
        } else {
            // Leave room for the run section and the hint line.
            let capacity_rows = (inner.height as usize)
                .saturating_sub(RECENT_RUNS + 4)
                .max(1);
            let start = crate::ui::selection::viewport_start(selected, rows.len(), capacity_rows);
            for (offset, row) in rows.iter().skip(start).take(capacity_rows).enumerate() {
                let mut style = Style::default();
                if row.degraded {
                    style = style.add_modifier(Modifier::DIM);
                }
                if start + offset == selected {
                    style = self.theme.selection();
                }
                lines.push(TLine::from(Span::styled(
                    clip(
                        &format!("{} · {}", row.label, row.detail),
                        (inner.width as usize).max(8),
                    ),
                    style,
                )));
            }

            self.push_recent_runs(&mut lines, inner.width as usize);

            lines.push(TLine::from(""));
            lines.push(TLine::from(Span::styled(
                "↑↓/jk browse · Enter run · r refresh",
                Style::default().add_modifier(Modifier::DIM),
            )));
        }

        f.render_widget(
            Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
            inner,
        );
    }

    /// Append the selected workflow's recent runs, from the cache.
    ///
    /// Reads nothing: the history is re-read when the selection or the
    /// catalogue changes, because a render pass must not touch the disk.
    fn push_recent_runs(&self, lines: &mut Vec<TLine<'static>>, width: usize) {
        if self.selected_workflow().is_none() {
            return;
        }
        lines.push(TLine::from(""));
        if let Some(error) = self.workflow_runs_error() {
            // Distinct from "no runs yet": one of these is fine and the other
            // is something an operator should go and look at.
            lines.push(TLine::from(Span::styled(
                clip(&format!("run history unreadable: {error}"), width.max(8)),
                Style::default().add_modifier(Modifier::DIM),
            )));
            return;
        }
        let runs = self.workflow_runs();

        if runs.is_empty() {
            lines.push(TLine::from(Span::styled(
                "No runs yet.",
                Style::default().add_modifier(Modifier::DIM),
            )));
            return;
        }

        lines.push(TLine::from(Span::styled(
            format!("recent runs · {}", runs.len()),
            Style::default().add_modifier(Modifier::DIM),
        )));
        for row in run_rows(runs).iter().take(RECENT_RUNS) {
            let mut style = Style::default();
            if row.degraded {
                style = style.add_modifier(Modifier::DIM);
            }
            lines.push(TLine::from(Span::styled(
                clip(&format!("  {} · {}", row.label, row.detail), width.max(8)),
                style,
            )));
        }
    }
}
