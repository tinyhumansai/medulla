//! Formatting workflow-run entries in the Sessions rail.
//!
//! A run is a distinct child row beneath the session that started it. This
//! module owns its elapsed-time calculation and presentation, keeping the rail
//! directory root focused on drawing and row dispatch.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span};

use super::super::super::super::rail::WorkflowRunRailRow;
use super::super::super::super::types::App;
use super::super::super::color;

/// How long a run has been going, or how long it took.
///
/// Settled runs end at their last report, while active ones end at `now`. A
/// backwards clock is clamped to the SDK's zero-duration representation.
pub(in crate::ui::app::render::sessions) fn workflow_run_elapsed(
    run: &medulla::control_socket::HarnessRun,
    now: i64,
) -> String {
    let end = if run.status.is_terminal() {
        run.updated_at
    } else {
        now.max(run.started_at)
    };
    medulla::ui::workflows::human_duration(end.saturating_sub(run.started_at).max(0) as u64)
}

impl App {
    /// Format a workflow run started by the session above it.
    pub(super) fn workflow_run_line(
        &self,
        row: &WorkflowRunRailRow,
        active: bool,
        now: i64,
    ) -> TLine<'static> {
        let branch = if row.last { "└" } else { "├" };
        let style = if active {
            self.theme.selection()
        } else {
            Style::default()
        };
        // The gear is the status surface: colour distinguishes each lifecycle
        // state, while blinking distinguishes a parked or moving run from one
        // that has settled. Keeping the signal on one glyph avoids repeating a
        // status word down every child row.
        let mut indicator_style = style.fg(color(row.run.status.color()));
        if !row.run.status.is_terminal() {
            indicator_style = indicator_style.add_modifier(Modifier::SLOW_BLINK);
        }
        TLine::from(vec![
            Span::styled(format!("   {branch} "), style),
            Span::styled("⚙", indicator_style),
            Span::styled(format!(" {}", self.workflow_run_title(&row.run)), style),
            Span::styled(
                format!(" · {}", workflow_run_elapsed(&row.run, now)),
                if active {
                    style
                } else {
                    style.add_modifier(Modifier::DIM)
                },
            ),
        ])
    }

    /// Resolve an installed workflow name, falling back to its reported id.
    fn workflow_run_title(&self, run: &medulla::control_socket::HarnessRun) -> String {
        self.workflows
            .iter()
            .find(|summary| summary.id == run.workflow_id)
            .map(|summary| summary.name.trim())
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| run.workflow_id.clone())
    }
}
