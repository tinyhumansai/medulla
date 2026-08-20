//! The Sessions rail: the list of hosts, sessions, and the runs beneath them.
//!
//! One cursor spans all of it (see [`rail`](crate::ui::app::rail)); this module
//! only draws what that cursor moves over, and formats each kind of row. The
//! per-lane glyph and status suffix live here too — they are the row's own
//! vocabulary, not the transcript's. Line-wrapping and path-shortening are
//! their own concern, kept in [`wrap`] so this file stays about *what* a row
//! says rather than how it is laid out across columns.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line as TLine, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::ui::agents::{AgentLane, TaskStatus};
use crate::worker::pty::ATTENTION_GLYPH;

use super::super::super::rail::RailRow;
use super::super::super::types::{App, RailHit};
use super::types::{Selection, SessionsPanes};

mod device_footer;
mod harness_line;
mod rows;
#[cfg(test)]
mod rows_tests;
mod state;
#[cfg(test)]
mod status_line_tests;
#[cfg(test)]
mod tests;
mod types;
mod workflow_run;
mod wrap;

use types::DeviceFooter;
pub(super) use workflow_run::workflow_run_elapsed;

/// The most content columns the Sessions rail ever takes.
///
/// The rail is a list of short labels beside the surface the operator is
/// actually reading. Sizing it to its widest row alone let one long harness
/// path — an absolute working directory is easily eighty columns — push the
/// transcript into a gutter. Rows that do not fit within this wrap onto a second
/// line instead of buying width nothing else needs.
pub(in crate::ui::app) const RAIL_MAX_CONTENT: usize = 36;

/// How far a wrapped row's continuation lines are indented, so a row that took
/// two lines still reads as one row rather than as two entries.
const CONT_INDENT: usize = 5;

/// Build the rail title from the rows, the lane inventory, and one attention
/// snapshot.
///
/// The count is of **sessions** — the rows the rail draws — because that is what
/// the rail now lists. It used to count agent rows, which the flattening
/// removed; counting the lanes instead would say how many things had been
/// dispatched to, which drops to zero on a quiet morning with three sessions
/// open. Running tasks still come from the lanes: that *is* a fact about traffic.
fn rail_title(rows: &[RailRow], lanes: &[AgentLane], waiting: usize) -> String {
    let running_tasks: usize = lanes
        .iter()
        .map(|lane| {
            lane.tasks
                .iter()
                .filter(|task| task.status == TaskStatus::Running)
                .count()
        })
        .sum();
    let sessions = rows
        .iter()
        .filter(|row| matches!(row, RailRow::Session(_)))
        .count();
    let mut title = if running_tasks > 0 {
        format!("Sessions · {sessions} · {running_tasks} running")
    } else {
        format!("Sessions · {sessions}")
    };
    if waiting > 0 {
        title.push_str(&format!(" · {ATTENTION_GLYPH} {waiting} waiting on you"));
    }
    title
}

impl App {
    /// Render every form of a row that can affect the rail's width.
    ///
    /// Harness fields may be visible only while selected, so measuring only the
    /// inactive form would allocate a rail that clips the row as soon as the
    /// cursor reaches it. Other row types only change style when selected.
    pub(super) fn rail_row_measurement_lines(
        &self,
        row: &RailRow,
        lanes: &[AgentLane],
    ) -> Vec<TLine<'static>> {
        let waiting_sessions = std::collections::HashSet::new();
        let now = medulla::clock::now_millis();
        match row {
            RailRow::Session(session) if session.local.is_some() && session.task.is_none() => {
                [false, true]
                    .into_iter()
                    .flat_map(|active| {
                        self.rail_row_lines(
                            row,
                            lanes,
                            active,
                            RAIL_MAX_CONTENT,
                            &waiting_sessions,
                            now,
                        )
                    })
                    .collect()
            }
            _ => self.rail_row_lines(row, lanes, false, RAIL_MAX_CONTENT, &waiting_sessions, now),
        }
    }

    /// Draw the rail list.
    pub(super) fn draw_sessions_rail(
        &mut self,
        f: &mut Frame,
        panes: &SessionsPanes,
        selection: &Selection,
    ) {
        // Collected once for the whole frame: the header count, the lane styling,
        // and the harness rows all answer from this one snapshot, so they cannot
        // disagree with each other — and the render thread takes the sessions
        // lock once rather than once per lane per task.
        let waiting_sessions = self
            .local_sessions
            .as_ref()
            .map(|h| h.sessions.waiting_sessions())
            .unwrap_or_default();
        // Every waiting harness on the device, including the ones inside lanes
        // rather than on rows of their own — the same number the tab badge
        // carries, so the two can never disagree.
        let waiting = App::count_waiting(&waiting_sessions, &self.harness_focus);
        let title = rail_title(&selection.rows, &selection.lanes, waiting);
        // The border says which half the keyboard is driving. Without it, Esc
        // moving focus to the rail is invisible until the next arrow press.
        let block = crate::ui::widgets::panel(&self.theme, title, self.sessions_rail_focused());
        let inner = block.inner(panes.rail);
        f.render_widget(block, panes.rail);

        // A row may occupy more than one line now, so the viewport is measured
        // in *lines* and each drawn line remembers the row it came from. Without
        // that map a click below a wrapped row would select its neighbour.
        let width = inner.width as usize;
        // Use a consistent timestamp across all rows so a waiting harness reports
        // the same elapsed time throughout the frame, not a new one for each row.
        let now = medulla::clock::now_millis();
        let mut lines: Vec<TLine> = Vec::new();
        let mut owners: Vec<RailHit> = Vec::new();
        let mut active_line = 0;
        let mut active_line_end = 0;
        for (index, row) in selection.rows.iter().enumerate() {
            if index == selection.active {
                active_line = lines.len();
            }
            for line in self.rail_row_lines(
                row,
                &selection.lanes,
                index == selection.active,
                width,
                &waiting_sessions,
                now,
            ) {
                lines.push(line);
                owners.push(RailHit::from_row(
                    row,
                    super::super::super::rail::rail_anchor(row, &selection.lanes),
                    index,
                ));
            }
            if index == selection.active {
                active_line_end = lines.len();
            }
        }

        let device_footer = DeviceFooter::prepare(self, width, inner.height as usize);
        let capacity = device_footer.navigation_capacity;
        let start =
            selected_row_viewport_start(active_line, active_line_end, lines.len(), capacity);
        // Clicks map against the navigation lines alone. Handing the footer's
        // rows to the hit test would turn a click on "Device RAM" into a
        // selection of whichever lane happened to share its offset.
        let nav_area = Rect {
            height: capacity.min(inner.height as usize) as u16,
            ..inner
        };
        self.hit_agents = Some((
            nav_area,
            owners.into_iter().skip(start).take(capacity).collect(),
        ));
        let mut view: Vec<TLine> = lines.into_iter().skip(start).take(capacity).collect();
        device_footer.append_to(&mut view, Style::default().fg(self.theme.accent));
        f.render_widget(Paragraph::new(Text::from(view)), inner);
    }
}

/// Center the selected row while keeping its final line visible when the full
/// multi-line row fits in the viewport.
fn selected_row_viewport_start(
    active_start: usize,
    active_end: usize,
    total: usize,
    capacity: usize,
) -> usize {
    let start = crate::ui::selection::viewport_start(active_start, total, capacity);
    let row_height = active_end.saturating_sub(active_start);
    if row_height <= capacity && active_end > start.saturating_add(capacity) {
        active_end.saturating_sub(capacity)
    } else {
        start
    }
}
