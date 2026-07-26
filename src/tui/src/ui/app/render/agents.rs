//! The Agents tab: the conversation surface. The lane list is on the left, the
//! selected lane's transcript (with a context bar) fills the right, and the
//! composer sits under it.
//!
//! This is where Chat went. Selecting the orchestrator lane and typing *is* the
//! conversation, so its transcript is the chat log rather than a list of
//! inference turns; selecting an agent shows that agent's own turns, and the
//! composer answers its open question instead of starting a new cycle. One
//! surface, because reading what an operation is doing and steering it were
//! never two jobs.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::ui::agents::{
    lane_lines, task_lines, AgentLane, AgentRole, AgentRow, Line as StyledLine, TaskState,
    TaskStatus,
};
use crate::ui::fleet::fleet_detail;
use crate::ui::harness::{budget_note, task_board_lines};
use crate::ui::meters;
use crate::ui::util::fmt_tokens;
use medulla::harness_contract::AgentBudgetMetadata;

use super::super::rail::RailRow;
use super::super::types::App;
use super::{chat_lines, color, styled_to_tline};

impl App {
    /// Draw the Agents tab: lane list and the selected lane/task transcript.
    pub(super) fn draw_agents(&mut self, f: &mut Frame, area: Rect) {
        let lanes = self.lanes();
        let rows = self.rail_rows();
        let active = self.agent_index.min(rows.len().saturating_sub(1));
        self.agent_index = active;
        let selected_row = rows.get(active);
        // A fleet row shows a declaration; everything else shows a transcript.
        let selected_node = self.selected_fleet_node();
        let active_lane_index = match selected_row {
            Some(RailRow::Agent(row)) => row.lane_index().unwrap_or(0),
            _ => 0,
        };
        let selected_task: Option<TaskState> = match selected_row {
            Some(RailRow::Agent(AgentRow::Sub { task, .. })) => Some(task.clone()),
            _ => None,
        };

        // Threads sit above the lanes, and only when there is more than one:
        // with a single thread the transcript title already names it.
        let threads = self.thread_rows();
        // Size the rail to its content rather than to a percentage: the labels
        // are short and fixed-ish, the transcript is what benefits from width.
        let widest = rows
            .iter()
            .map(|row| self.rail_row_line(row, &lanes, false).width())
            .chain(threads.iter().map(|line| line.width()))
            .max()
            .unwrap_or(0);
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(crate::ui::multi_pane::sidebar_width(area.width, widest)),
                Constraint::Min(0),
            ])
            .split(area);
        let left = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(if threads.len() > 1 {
                    (threads.len() as u16 + 2).min(area.height / 3)
                } else {
                    0
                }),
                Constraint::Min(0),
            ])
            .split(cols[0]);
        // The composer lives inside the right column, under the transcript it
        // belongs to, and grows with the draft.
        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(self.composer_height()),
            ])
            .split(cols[1]);

        let running_tasks: usize = lanes
            .iter()
            .map(|l| {
                l.tasks
                    .iter()
                    .filter(|t| t.status == TaskStatus::Running)
                    .count()
            })
            .sum();
        let agent_count = lanes.iter().filter(|l| !l.role.is_function()).count();
        let title = if running_tasks > 0 {
            format!("Agents · {agent_count} · {running_tasks} running")
        } else {
            format!("Agents · {agent_count}")
        };
        if threads.len() > 1 {
            self.draw_thread_strip(f, left[0], &threads);
        } else {
            self.hit_threads = None;
        }
        let block = self.panel(title);
        let inner = block.inner(left[1]);
        f.render_widget(block, left[1]);
        let capacity = (inner.height as usize).max(1);
        let window_start = crate::ui::selection::viewport_start(active, rows.len(), capacity);
        self.hit_agents = Some((inner, window_start));
        let mut lines: Vec<TLine> = Vec::new();
        for (offset, row) in rows.iter().skip(window_start).take(capacity).enumerate() {
            let idx = window_start + offset;
            lines.push(self.rail_row_line(row, &lanes, idx == active));
        }
        f.render_widget(Paragraph::new(Text::from(lines)), inner);

        // Transcript pane.
        let lane = lanes.get(active_lane_index);
        let pane_width = ((cols[1].width as usize).saturating_sub(4)).max(24);
        let on_orchestrator = selected_node.is_none()
            && lane
                .map(|l| l.role == AgentRole::Orchestrator)
                .unwrap_or(true);
        let content_lines: Vec<StyledLine> = if let Some(node) = &selected_node {
            fleet_detail(
                &self.fleet_capacity(),
                &self.fleet_roster(),
                &node.key,
                pane_width,
            )
        } else if let Some(t) = &selected_task {
            task_lines(t, pane_width)
        } else if on_orchestrator {
            // The orchestrator lane is the conversation: show what was said, not
            // the model calls that said it. The calls stay in Settings › Trace.
            chat_lines(&self.snapshot.events, pane_width)
        } else {
            lane_lines(lane, pane_width)
        };
        let title = if let Some(node) = &selected_node {
            format!("{} · {}", node.kind.label(), node.label)
        } else if let Some(t) = &selected_task {
            format!(
                "{} › {} · {} turns",
                lane.map(|l| l.label.as_str()).unwrap_or("task"),
                t.task_id,
                t.turns
            )
        } else if on_orchestrator {
            let thread = self
                .snapshot
                .threads
                .get(self.active_thread_idx())
                .map(|t| t.name.clone())
                .unwrap_or_else(|| "main".into());
            format!(
                "orchestrator · {thread} · {} turns",
                self.snapshot.messages.len().div_ceil(2)
            )
        } else if let Some(l) = lane {
            format!("{} · {} turns", l.label, l.turns.len())
        } else {
            "Transcript".into()
        };
        let block = self.panel(title);
        let inner = block.inner(right[0]);
        f.render_widget(block, right[0]);
        let mut header: Vec<TLine> = Vec::new();
        // Harness task board: session-wide, shown only when the backend surfaces a
        // `HarnessStatus`. Degrades to nothing (empty vec) when absent or empty.
        if let Some(status) = &self.snapshot.harness {
            for line in task_board_lines(status, pane_width) {
                header.push(styled_to_tline(&line));
            }
        }
        // Read-only seat budget for the selected lane, when its descriptor carries
        // a `metadata.budget` stamp. Seat CRUD stays a backend REST concern.
        if let Some(budget) = lane
            .and_then(|l| l.descriptor.as_ref())
            .and_then(|d| AgentBudgetMetadata::from_metadata(&d.metadata))
        {
            header.push(TLine::from(Span::styled(
                format!("seat {}", budget_note(&budget)),
                Style::default().fg(if budget.exhausted {
                    Color::Red
                } else {
                    Color::Magenta
                }),
            )));
        }
        // Where this lane runs, and how hard: the placement chip, then compact
        // meters for the machine's memory and load and for the lane's own
        // context window. Every reading is omitted rather than zeroed when it
        // was not reported — a bar at 0% claims a measurement nobody took.
        if selected_node.is_none() {
            let capacity = self.fleet_capacity();
            if let Some(descriptor) = lane.and_then(|l| l.descriptor.as_ref()) {
                let placement = capacity.placement(descriptor);
                let chip = [
                    placement.host.map(|h| h.name.clone()),
                    placement.harness.map(|h| h.kind.clone()),
                    placement.workspace.map(|w| w.path.clone()),
                    placement
                        .template
                        .map(|t| format!("via {}", t.name.clone().unwrap_or_else(|| t.id.clone()))),
                ]
                .into_iter()
                .flatten()
                .filter(|part| !part.trim().is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
                if !chip.is_empty() {
                    header.push(TLine::from(Span::styled(
                        chip,
                        Style::default().fg(Color::Cyan),
                    )));
                }
                if let Some(resources) = placement.host.and_then(|h| h.resources.as_ref()) {
                    for line in [
                        meters::cpu_meter(resources),
                        meters::memory_meter(resources),
                        meters::disk_line(resources),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        header.push(styled_to_tline(&line));
                    }
                }
            }
            if let Some(lane) = lane {
                let window = self.loaded.config.medulla.context_window() as i64;
                if let Some(line) = meters::context_meter(&lane.usage, window) {
                    header.push(styled_to_tline(&line));
                }
            }
        }
        // Reserve the last row for the status line (scroll notice or spinner),
        // or a below-fold notice would itself fall below the fold.
        let capacity = (inner.height as usize)
            .saturating_sub(header.len() + 1)
            .max(4);
        let max_scroll = content_lines.len().saturating_sub(capacity);
        // The two transcripts keep their own scroll positions, so switching
        // lanes and switching back does not lose your place in either.
        let eff = if on_orchestrator {
            let eff = self.chat_scroll.min(max_scroll);
            self.chat_scroll = eff;
            eff
        } else {
            self.agent_scroll.min(max_scroll)
        };
        let end = content_lines.len() - eff;
        let view = &content_lines[end.saturating_sub(capacity)..end];
        let mut out = header;
        if view.is_empty() {
            out.push(TLine::from(Span::styled(
                "No messages yet — type below to start.",
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        out.extend(view.iter().map(styled_to_tline));
        if eff > 0 {
            out.push(TLine::from(Span::styled(
                format!("↑ {eff} more line(s) below · k to catch up"),
                Style::default().add_modifier(Modifier::DIM),
            )));
        } else if on_orchestrator && self.snapshot.running {
            let calls = crate::ui::stream::running_calls(&self.snapshot.events);
            let msg = if calls > 0 {
                format!(
                    "thinking · {calls} model call{} in flight",
                    if calls == 1 { "" } else { "s" }
                )
            } else {
                "working…".into()
            };
            out.push(TLine::from(Span::styled(
                format!(
                    "{} {msg}",
                    crate::ui::util::SPINNER[self.frame % crate::ui::util::SPINNER.len()],
                ),
                Style::default().fg(Color::Yellow),
            )));
        }
        f.render_widget(Paragraph::new(Text::from(out)), inner);

        // Composer: what it submits depends on the cursor, so it says so.
        self.draw_agent_composer(f, right[1], &selected_task, lane);
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

    /// The composer's height: its caption row, the draft's lines, and the border.
    pub(super) fn composer_height(&self) -> u16 {
        (self.draft.text.split('\n').count() as u16).max(1) + 3
    }

    /// Draw the composer with a caption naming its target.
    fn draw_agent_composer(
        &mut self,
        f: &mut Frame,
        area: Rect,
        selected_task: &Option<TaskState>,
        lane: Option<&AgentLane>,
    ) {
        let target = match selected_task
            .as_ref()
            .filter(|task| task.question_id.is_some())
        {
            Some(task) => format!("answering {}", task.task_id),
            None => lane
                .filter(|l| l.role != AgentRole::Orchestrator)
                .map(|l| format!("{} · Enter still instructs the orchestrator", l.label))
                .unwrap_or_else(|| "orchestrator".into()),
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(area);
        f.render_widget(
            Paragraph::new(TLine::from(Span::styled(
                format!(" › {target}"),
                Style::default().add_modifier(Modifier::DIM),
            ))),
            rows[0],
        );
        self.draw_composer(f, rows[1]);
    }

    /// Format one rail row: a lane row, the fleet divider, or a fleet node.
    pub(super) fn rail_row_line(
        &self,
        row: &RailRow,
        lanes: &[AgentLane],
        active: bool,
    ) -> TLine<'static> {
        match row {
            RailRow::Agent(row) => self.agent_row_line(row, lanes, active),
            RailRow::Divider(text) => TLine::from(Span::styled(
                text.to_string(),
                Style::default().add_modifier(Modifier::DIM),
            )),
            RailRow::Fleet(node) => {
                let mut style = Style::default().fg(color(node.kind.color()));
                if node.degraded {
                    style = style.add_modifier(Modifier::DIM);
                }
                if active {
                    style = self.theme.selection();
                }
                let indent = "  ".repeat(node.depth);
                let text = if node.detail.is_empty() {
                    format!("{indent}{}", node.label)
                } else {
                    format!("{indent}{} · {}", node.label, node.detail)
                };
                TLine::from(Span::styled(text, style))
            }
        }
    }

    /// Format one Agents-list row (separator, "more", sub-task, or lane).
    pub(super) fn agent_row_line(
        &self,
        row: &AgentRow,
        lanes: &[AgentLane],
        active: bool,
    ) -> TLine<'static> {
        match row {
            AgentRow::Separator => TLine::from(Span::styled(
                "── functions ──",
                Style::default().add_modifier(Modifier::DIM),
            )),
            AgentRow::More { hidden, .. } => TLine::from(Span::styled(
                format!("   └ +{hidden} more"),
                Style::default().add_modifier(Modifier::DIM),
            )),
            AgentRow::Sub { task, last, .. } => {
                let branch = if *last { "└" } else { "├" };
                let mut style = Style::default();
                if active {
                    style = self.theme.selection();
                }
                let status_style = if active {
                    style
                } else {
                    style.fg(color(task.status.color()))
                };
                TLine::from(vec![
                    Span::styled(format!("   {branch} {} · ", task.task_id), style),
                    Span::styled(task.status.label().to_string(), status_style),
                    Span::styled(format!(" · {} turns", task.turns), style),
                ])
            }
            AgentRow::Lane { lane_index } => {
                let Some(item) = lanes.get(*lane_index) else {
                    return TLine::from("");
                };
                let window = self.loaded.config.medulla.context_window() as i64;
                let is_fn = item.role.is_function();
                let ctx = match item.context_tokens {
                    None => String::new(),
                    Some(used) if item.role == AgentRole::Agent => {
                        format!(" · ctx {}", fmt_tokens(used))
                    }
                    Some(used) => format!(
                        " · ctx {}/{} {}%",
                        fmt_tokens(used),
                        fmt_tokens(window),
                        ((used as f64 / window as f64) * 100.0).round() as i64
                    ),
                };
                let marker = self.lane_marker(item, is_fn);
                let state = self.lane_state(item);
                let sessions_note = if let Some(aid) = &item.agent_id {
                    let list = self.snapshot.sessions.get(aid).cloned().unwrap_or_default();
                    if list.is_empty() {
                        String::new()
                    } else {
                        let live = list.iter().filter(|s| s.state != "ended").count();
                        format!(" · {}/{} sess", live, list.len())
                    }
                } else {
                    String::new()
                };
                let mut style = Style::default().fg(color(item.role.color()));
                if is_fn {
                    style = style.add_modifier(Modifier::DIM);
                }
                if active {
                    style = self.theme.selection();
                }
                crate::ui::agent_lane::line(
                    marker,
                    item.label.clone(),
                    format!(" · {}{ctx}{state}{sessions_note}", item.turns.len()),
                    style,
                )
            }
        }
    }

    /// The presence/status glyph for a lane row.
    pub(super) fn lane_marker(&self, item: &AgentLane, is_fn: bool) -> &'static str {
        if is_fn {
            "ƒ"
        } else if item.role != AgentRole::Agent {
            "●"
        } else if item.session_id.is_some() {
            let state = self.session_state(item);
            match state.as_deref() {
                Some("ended") => "○",
                _ => "●",
            }
        } else if let Some(aid) = &item.agent_id {
            match self.snapshot.presence.get(aid) {
                Some(p) => {
                    if p.online {
                        "●"
                    } else {
                        "○"
                    }
                }
                None if item.descriptor.is_some() => "◌",
                None => "◆",
            }
        } else if item.descriptor.is_some() {
            "◌"
        } else {
            "◆"
        }
    }

    /// The state of the session backing a lane, if any.
    pub(super) fn session_state(&self, item: &AgentLane) -> Option<String> {
        let (sid, pid) = (item.session_id.as_ref()?, item.parent_agent_id.as_ref()?);
        self.snapshot
            .sessions
            .get(pid)?
            .iter()
            .find(|s| &s.id == sid)
            .map(|s| s.state.clone())
    }

    /// A short human-readable state suffix for a lane row.
    pub(super) fn lane_state(&self, item: &AgentLane) -> String {
        if item.session_id.is_some() {
            let s = self.session_state(item);
            match s.as_deref() {
                Some("ended") => " · inactive".into(),
                Some(other) => format!(" · {other}"),
                None => " · …".into(),
            }
        } else if item.role == AgentRole::Agent {
            if item.active_tasks > 0 {
                " · busy".into()
            } else if item.turns.is_empty() {
                " · idle".into()
            } else {
                String::new()
            }
        } else {
            String::new()
        }
    }
}
