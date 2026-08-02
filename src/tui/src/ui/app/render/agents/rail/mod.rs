//! The Agents rail: the threads strip, and the list of lanes, fleet rows, and
//! agent templates beneath it.
//!
//! One cursor spans all of it (see [`rail`](crate::ui::app::rail)); this module
//! only draws what that cursor moves over, and formats each kind of row. The
//! per-lane glyph and status suffix live here too — they are the row's own
//! vocabulary, not the transcript's. Line-wrapping and path-shortening are
//! their own concern, kept in [`wrap`] so this file stays about *what* a row
//! says rather than how it is laid out across columns.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::ui::agents::{AgentLane, TaskStatus};
use crate::worker::pty::ATTENTION_GLYPH;

use super::super::super::rail::{RailRow, NEW_HARNESS_LABEL};
use super::super::super::types::App;
use super::super::color;
use super::types::{AgentsPanes, Selection};

#[cfg(test)]
mod attention_tests;
mod device_footer;
mod harness_line;
mod rows;
mod state;
mod status;
#[cfg(test)]
mod status_line_tests;
#[cfg(test)]
mod tests;
mod wrap;

use device_footer::DeviceFooter;
use wrap::wrap_line;

/// The most content columns the Agents rail ever takes.
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

/// Build the rail title from the lane inventory and one attention snapshot.
fn rail_title(lanes: &[AgentLane], waiting: usize) -> String {
    let running_tasks: usize = lanes
        .iter()
        .map(|lane| {
            lane.tasks
                .iter()
                .filter(|task| task.status == TaskStatus::Running)
                .count()
        })
        .sum();
    let agents = lanes.iter().filter(|lane| !lane.role.is_function()).count();
    let mut title = if running_tasks > 0 {
        format!("Agents · {agents} · {running_tasks} running")
    } else {
        format!("Agents · {agents}")
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
            RailRow::Harness(_) => [false, true]
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
                .collect(),
            _ => self.rail_row_lines(row, lanes, false, RAIL_MAX_CONTENT, &waiting_sessions, now),
        }
    }

    /// Draw the threads strip and the rail list under it.
    pub(super) fn draw_agents_rail(
        &mut self,
        f: &mut Frame,
        panes: &AgentsPanes,
        selection: &Selection,
    ) {
        let threads = self.thread_rows();
        if threads.len() > 1 {
            self.draw_thread_strip(f, panes.threads, &threads);
        } else {
            self.hit_threads = None;
        }

        // Collected once for the whole frame: the header count, the lane styling,
        // and the harness rows all answer from this one snapshot, so they cannot
        // disagree with each other — and the render thread takes the sessions
        // lock once rather than once per lane per task.
        let waiting_sessions = self
            .harnesses
            .as_ref()
            .map(|h| h.sessions.waiting_sessions())
            .unwrap_or_default();
        // Every waiting harness on the device, including the ones inside lanes
        // rather than on rows of their own — the same number the tab badge
        // carries, so the two can never disagree.
        let waiting = App::count_waiting(&waiting_sessions, &self.harness_focus);
        let title = rail_title(&selection.lanes, waiting);
        // The border says which half the keyboard is driving. Without it, Esc
        // moving focus to the rail is invisible until the next arrow press.
        let block = crate::ui::widgets::panel(&self.theme, title, self.agents_rail_focused());
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
        let mut owners: Vec<usize> = Vec::new();
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
                owners.push(index);
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
            owners.iter().skip(start).take(capacity).copied().collect(),
        ));
        let mut view: Vec<TLine> = lines.into_iter().skip(start).take(capacity).collect();
        device_footer.append_to(&mut view, Style::default().fg(self.theme.accent));
        f.render_widget(Paragraph::new(Text::from(view)), inner);
    }

    /// One row per open thread, with its running/attention badges. Built apart
    /// from the draw so the rail can be sized to it.
    pub(super) fn thread_rows(&self) -> Vec<TLine<'static>> {
        self.snapshot
            .threads
            .iter()
            .map(|thread| {
                let marker = if thread.running { "▶" } else { "●" };
                let mut badges = Vec::new();
                if thread.running_tasks > 0 {
                    badges.push(format!("{} run", thread.running_tasks));
                }
                if thread.attention > 0 {
                    badges.push(format!("{}⚠", thread.attention));
                }
                let badge = if badges.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", badges.join(" "))
                };
                let mut style = Style::default();
                if thread.running {
                    style = style.fg(Color::Yellow);
                }
                if thread.id == self.snapshot.active_thread_id {
                    style = self.theme.selection();
                }
                TLine::from(Span::styled(
                    format!("{marker} {} · {}t{badge}", thread.name, thread.turns),
                    style,
                ))
            })
            .collect()
    }

    /// Draw the threads strip above the lane list.
    fn draw_thread_strip(&mut self, f: &mut Frame, area: Rect, threads: &[TLine<'static>]) {
        let block = self.panel(format!("Threads · {}", threads.len()));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let capacity = (inner.height as usize).max(1);
        let active = self.active_thread_idx();
        let window_start = crate::ui::selection::viewport_start(active, threads.len(), capacity);
        self.hit_threads = Some((inner, window_start));
        let view: Vec<TLine> = threads
            .iter()
            .skip(window_start)
            .take(capacity)
            .cloned()
            .collect();
        f.render_widget(Paragraph::new(Text::from(view)), inner);
    }

    /// Format one rail row as the lines it occupies, wrapped to `width`.
    ///
    /// Most rows fit on one. A harness row is two or three by construction: its
    /// provider and who holds it on the first, its working directory under
    /// that — the directory is the only thing telling two harnesses of the same
    /// provider apart, so it has to be readable, and reading it used to cost
    /// the transcript however many columns the path happened to want. Anything
    /// else that overruns — a lane labelled with a full tiny.place address, a
    /// task row carrying a long work chip — wraps rather than being cut off at
    /// the border or widening the rail for everyone.
    pub(super) fn rail_row_lines(
        &self,
        row: &RailRow,
        lanes: &[AgentLane],
        active: bool,
        width: usize,
        waiting_sessions: &std::collections::HashSet<String>,
        now: i64,
    ) -> Vec<TLine<'static>> {
        match row {
            RailRow::Harness(session) => self.own_harness_lines(session, active, width, now),
            other => wrap_line(
                &self.rail_row_line(other, lanes, active, waiting_sessions, now),
                width,
                CONT_INDENT,
            ),
        }
    }

    /// Format one single-line rail row.
    pub(super) fn rail_row_line(
        &self,
        row: &RailRow,
        lanes: &[AgentLane],
        active: bool,
        waiting_sessions: &std::collections::HashSet<String>,
        now: i64,
    ) -> TLine<'static> {
        match row {
            RailRow::Agent(row) => self.agent_row_line(row, lanes, active, waiting_sessions),
            RailRow::NewHarness => self.new_harness_line(active),
            RailRow::HarnessSeparator => TLine::from(Span::styled(
                "── your harnesses ──",
                Style::default().add_modifier(Modifier::DIM),
            )),
            // Only reached through `rail_row_lines`, which draws a harness over
            // several lines; kept total so measurement can call either.
            RailRow::Harness(row) => self
                .own_harness_lines(row, active, RAIL_MAX_CONTENT, now)
                .into_iter()
                .next()
                .unwrap_or_default(),
        }
    }

    /// Format the `+ New harness` action row.
    ///
    /// Drawn as a button rather than as another list entry — bold and coloured,
    /// with its chord beside it — because it is the one row on the rail that
    /// *does* something rather than selecting something.
    fn new_harness_line(&self, active: bool) -> TLine<'static> {
        let style = if active {
            self.theme.selection()
        } else {
            Style::default()
                .fg(color("cyan"))
                .add_modifier(Modifier::BOLD)
        };
        TLine::from(vec![
            Span::styled(format!(" {NEW_HARNESS_LABEL} "), style),
            Span::styled(" ⏎ / ^T", Style::default().add_modifier(Modifier::DIM)),
        ])
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
