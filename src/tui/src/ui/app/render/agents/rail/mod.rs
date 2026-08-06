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

use super::super::super::rail::{RailRow, NEW_AGENT_LABEL, NEW_SESSION_LABEL};
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
mod types;
mod wrap;

use types::DeviceFooter;
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

/// Build the rail title from the tree, the lane inventory, and one attention
/// snapshot.
///
/// The count is of **agents** — the rows of the tree — not of lanes. A lane is
/// folded from traffic, so counting lanes said how many things had been
/// dispatched to, which is not what "Agents · 3" claims and drops to zero on a
/// machine with three declared agents and a quiet morning. Running tasks still
/// come from the lanes: that *is* a fact about traffic.
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
    let agents = rows
        .iter()
        .filter(|row| matches!(row, RailRow::Agent(_)))
        .count();
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
                if thread.attention > 0 {
                    style = style.fg(self.theme.attention);
                    if self.theme.attention_blink {
                        style = style.add_modifier(Modifier::SLOW_BLINK);
                    }
                }
                if thread.id == self.snapshot.active_thread_id {
                    style = if thread.attention > 0 {
                        style.bg(self.theme.primary).add_modifier(Modifier::BOLD)
                    } else {
                        self.theme.selection()
                    };
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
    /// else that overruns — a lane labelled with a full link address, a
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
            // A session this device runs and no task describes is the operator's
            // own: it gets the multi-line status-line treatment, because its
            // working directory is the only thing telling two of them apart.
            //
            // A name goes above that rather than into it. The status line is
            // configurable and describes the *harness*; the name is what the
            // person who opened the session called it, and it is the first thing
            // they look for.
            RailRow::Session(session) if session.task.is_none() => {
                let Some(local) = &session.local else {
                    return Vec::new();
                };
                let mut lines = Vec::new();
                if let Some(name) = session.name() {
                    let style = if active {
                        self.theme.selection()
                    } else {
                        Style::default().fg(color("cyan"))
                    };
                    lines.extend(wrap_line(
                        &TLine::from(Span::styled(format!("  {name}"), style)),
                        width,
                        CONT_INDENT,
                    ));
                }
                lines.extend(self.own_session_lines(local, active, width, now));
                lines
            }
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
            RailRow::Lane(row) => self.agent_row_line(row, lanes, active, waiting_sessions),
            RailRow::Host(host) => TLine::from(Span::styled(
                format!("▸ {}", host.label),
                Style::default()
                    .fg(color("blue"))
                    .add_modifier(Modifier::BOLD),
            )),
            RailRow::Agent(agent) => {
                self.declared_agent_line(agent, lanes, active, waiting_sessions)
            }
            RailRow::NewAgent => self.new_agent_line(active),
            // Same shape as the lane list's `── functions ──`, so the two
            // headings on one rail read as the same kind of thing.
            RailRow::AgentsHeader => TLine::from(Span::styled(
                "── agents ──",
                Style::default().add_modifier(Modifier::DIM),
            )),
            RailRow::NewSession { .. } => self.new_session_line(active),
            RailRow::WorkflowRun(run) => self.workflow_run_line(run, active),
            RailRow::Session(session) => match (&session.task, &session.local) {
                (Some(task), _) => self.agent_row_line(
                    &crate::ui::agents::AgentRow::Sub {
                        lane_index: session.lane_index.unwrap_or(0),
                        task: task.clone(),
                        last: session.last,
                    },
                    lanes,
                    active,
                    waiting_sessions,
                ),
                // Only reached through `rail_row_lines`, which draws a local
                // session over several lines; kept total so measurement can call
                // either.
                (None, Some(local)) => self
                    .own_session_lines(local, active, RAIL_MAX_CONTENT, now)
                    .into_iter()
                    .next()
                    .unwrap_or_default(),
                (None, None) => TLine::from(""),
            },
        }
    }

    /// Format a workflow run started by the session above it.
    ///
    /// Deliberately shaped like a task sublane — the same branch glyph and
    /// `· status` suffix — because that is what it is from the operator's side:
    /// work this session set going, nested under it. The workflow's name leads,
    /// since the run id means nothing until you go looking for it, and the
    /// latest thing the run reported follows as a dim tail so a long-running
    /// step still shows movement.
    fn workflow_run_line(
        &self,
        row: &super::super::super::rail::WorkflowRunRailRow,
        active: bool,
    ) -> TLine<'static> {
        let branch = if row.last { "└" } else { "├" };
        let style = if active {
            self.theme.selection()
        } else {
            Style::default()
        };
        let status_style = if active {
            style
        } else {
            Style::default().fg(color(row.run.status.color()))
        };
        let detail = row
            .run
            .detail
            .as_deref()
            .map(str::trim)
            .filter(|detail| !detail.is_empty())
            .map(|detail| format!(" · {detail}"))
            .unwrap_or_default();
        TLine::from(vec![
            Span::styled(format!("   {branch} ⚙ {} · ", row.run.workflow_id), style),
            Span::styled(row.run.status.label().to_string(), status_style),
            Span::styled(
                detail,
                if active {
                    style
                } else {
                    style.add_modifier(Modifier::DIM)
                },
            ),
        ])
    }

    /// Format an agent row.
    ///
    /// An agent with a lane keeps the lane's own line — its turn count, context
    /// window, and live state are what an operator reads it for. A *declared*
    /// agent with no traffic has none of that, so it says what it is instead:
    /// its harness and the folder it works in, dimmed, because "declared and
    /// idle" should not compete with the rows that are doing something.
    fn declared_agent_line(
        &self,
        agent: &super::super::super::rail::AgentRailRow,
        lanes: &[AgentLane],
        active: bool,
        waiting_sessions: &std::collections::HashSet<String>,
    ) -> TLine<'static> {
        if let Some(lane_index) = agent.lane_index {
            return self.agent_row_line(
                &crate::ui::agents::AgentRow::Lane { lane_index },
                lanes,
                active,
                waiting_sessions,
            );
        }
        // The name carries the row; the harness and directory qualify it. Both
        // were dim, which left an idle agent with nothing to read it by — the
        // whole row receded, including the one word that identifies it. Only the
        // qualifier recedes now.
        let (name_style, detail_style) = if active {
            (self.theme.selection(), self.theme.selection())
        } else {
            (
                Style::default().fg(color("cyan")),
                Style::default().add_modifier(Modifier::DIM),
            )
        };
        let detail = match (agent.harness(), agent.workspace()) {
            (Some(harness), Some(workspace)) => format!(
                " · {harness} · {}",
                crate::ui::util::clip_left(workspace, 24)
            ),
            (Some(harness), None) => format!(" · {harness}"),
            _ => String::new(),
        };
        TLine::from(vec![
            Span::styled(format!("○ {}", agent.label()), name_style),
            Span::styled(detail, detail_style),
        ])
    }

    /// Format the `+ New agent` action row.
    ///
    /// Drawn as a button rather than as another list entry — bold and coloured,
    /// with its chord beside it — because it is the one row on the rail that
    /// *does* something rather than selecting something.
    fn new_agent_line(&self, active: bool) -> TLine<'static> {
        let style = if active {
            self.theme.selection()
        } else {
            Style::default()
                .fg(color("cyan"))
                .add_modifier(Modifier::BOLD)
        };
        TLine::from(vec![
            Span::styled(format!(" {NEW_AGENT_LABEL} "), style),
            Span::styled(" ⏎", Style::default().add_modifier(Modifier::DIM)),
        ])
    }

    /// Format the `+ new session` action row that closes an agent's group.
    ///
    /// Drawn as the group's last leaf — the same `└` the last session would have
    /// carried — so it reads as part of that agent rather than as a second
    /// machine-level button beside `+ New agent`. It carries the `^T` hint the
    /// machine-level button used to, because `Ctrl-T` on a row belonging to an
    /// agent opens exactly this flow.
    fn new_session_line(&self, active: bool) -> TLine<'static> {
        let style = if active {
            self.theme.selection()
        } else {
            Style::default().fg(color("cyan"))
        };
        TLine::from(vec![
            Span::styled(format!("   └ {NEW_SESSION_LABEL}"), style),
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
