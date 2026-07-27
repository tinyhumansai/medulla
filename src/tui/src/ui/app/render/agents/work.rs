//! The Work pane: what the selected agent is doing, as its own harness sees it.
//!
//! The transcript answers "what has been said"; this answers "what is being
//! worked on" — the goal, the todo list, the sub-agents running underneath, and
//! the files being changed. Those are the four things another harness's screen
//! shows and this one could not, which is the difference between watching a
//! fleet and reading its logs.
//!
//! The pane appears only when the selection has a snapshot and the terminal is
//! wide enough to spare the columns; otherwise the transcript keeps the whole
//! width and the same information stays reachable as the header headline.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use medulla::harness_work::WorkSnapshot;

use super::super::super::types::App;
use super::super::styled_to_tline;
use super::types::Selection;

/// Columns the work pane takes when it is shown.
pub(super) const WORK_PANE_WIDTH: u16 = 34;
/// Total width below which the pane is not worth its columns: taking a third of
/// an 80-column terminal would leave the transcript unreadable, and the header
/// headline already carries the one line that matters most.
pub(super) const MIN_WIDTH_FOR_WORK_PANE: u16 = 110;

impl App {
    /// The work snapshot the current selection should show, if any.
    ///
    /// A selected task shows its own; a selected lane shows the freshest across
    /// its tasks. Empty snapshots are treated as absent so the pane never opens
    /// on nothing.
    pub(super) fn selected_work<'a>(&self, selection: &'a Selection) -> Option<&'a WorkSnapshot> {
        if selection.node.is_some() || selection.on_orchestrator {
            return None;
        }
        selection
            .task
            .as_ref()
            .and_then(|task| task.work.as_deref())
            .or_else(|| selection.lane().and_then(|lane| lane.work.as_deref()))
            .filter(|work| !work.is_empty())
    }

    /// Draw the work panel into `area`.
    pub(super) fn draw_agents_work(&mut self, f: &mut Frame, area: Rect, work: &WorkSnapshot) {
        let (done, total) = work.todo_progress();
        let title = if total > 0 {
            format!("Work · {done}/{total}")
        } else {
            "Work".to_string()
        };
        let block = self.panel(title);
        let inner = block.inner(area);
        f.render_widget(block, area);

        let width = (inner.width as usize).max(8);
        let lines = crate::ui::work::work_lines(work, width);
        let capacity = inner.height as usize;
        // The head of the panel is its most important part — the goal and the
        // live todo — so an overflow drops the tail rather than scrolling away
        // from what the operator opened it for.
        let mut out: Vec<TLine> = lines
            .iter()
            .take(capacity.saturating_sub(1))
            .map(styled_to_tline)
            .collect();
        if lines.len() > out.len() {
            out.push(TLine::from(Span::styled(
                format!("… {} more line(s)", lines.len() - out.len()),
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        f.render_widget(Paragraph::new(Text::from(out)), inner);
    }
}
