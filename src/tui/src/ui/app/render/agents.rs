//! The Agents tab: the agent/task lane list on the left, the selected lane's or
//! task's transcript (with a context bar) on the right, and the row/marker
//! formatting helpers those two panes share.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::ui::agents::{
    lane_lines, task_lines, AgentLane, AgentRole, AgentRow, Line as StyledLine, TaskState,
    TaskStatus,
};
use crate::ui::harness::{budget_note, task_board_lines};
use crate::ui::util::fmt_tokens;
use medulla::harness_contract::AgentBudgetMetadata;

use super::super::types::App;
use super::{color, styled_to_tline};

impl App {
    /// Draw the Agents tab: lane list and the selected lane/task transcript.
    pub(super) fn draw_agents(&mut self, f: &mut Frame, area: Rect) {
        let lanes = self.lanes();
        let rows = self.agent_rows();
        let active = self.agent_index.min(rows.len().saturating_sub(1));
        self.agent_index = active;
        let selected_row = rows.get(active);
        let active_lane_index = selected_row.and_then(|r| r.lane_index()).unwrap_or(0);
        let selected_task: Option<TaskState> = match selected_row {
            Some(AgentRow::Sub { task, .. }) => Some(task.clone()),
            _ => None,
        };

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(36), Constraint::Min(0)])
            .split(area);

        let running_tasks: usize = lanes
            .iter()
            .map(|l| {
                l.tasks
                    .iter()
                    .filter(|t| t.status == TaskStatus::Running)
                    .count()
            })
            .sum();
        let lane_count = lanes
            .iter()
            .filter(|lane| lane.role == AgentRole::Worker)
            .count();
        let max_lanes = self.loaded.config.workflow.max_lanes;
        let title = if running_tasks > 0 {
            format!("Agents · lanes {lane_count}/{max_lanes} · {running_tasks} running")
        } else {
            format!("Agents · lanes {lane_count}/{max_lanes}")
        };
        let block = self.panel(title);
        let inner = block.inner(cols[0]);
        f.render_widget(block, cols[0]);
        let capacity = (inner.height as usize).max(1);
        let window_start = active
            .saturating_sub(capacity / 2)
            .min(rows.len().saturating_sub(capacity));
        self.hit_agents = Some((inner, window_start));
        let mut lines: Vec<TLine> = Vec::new();
        for (offset, row) in rows.iter().skip(window_start).take(capacity).enumerate() {
            let idx = window_start + offset;
            lines.push(self.agent_row_line(row, &lanes, idx == active));
        }
        f.render_widget(Paragraph::new(Text::from(lines)), inner);

        // Transcript pane.
        let lane = lanes.get(active_lane_index);
        let pane_width = ((cols[1].width as usize).saturating_sub(4)).max(24);
        let content_lines: Vec<StyledLine> = if let Some(t) = &selected_task {
            task_lines(t, pane_width)
        } else {
            lane_lines(lane, pane_width)
        };
        let title = if let Some(t) = &selected_task {
            format!(
                "{} › {} · {} turns",
                lane.map(|l| l.label.as_str()).unwrap_or("task"),
                t.task_id,
                t.turns
            )
        } else if let Some(l) = lane {
            format!("{} · {} turns", l.label, l.turns.len())
        } else {
            "Transcript".into()
        };
        let block = self.panel(title);
        let inner = block.inner(cols[1]);
        f.render_widget(block, cols[1]);
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
        if let Some(lane) = lane {
            let report = self.lane_guard_report();
            let badges = report.badges(&lane.key);
            if !badges.is_empty() {
                header.push(TLine::from(Span::styled(
                    badges
                        .into_iter()
                        .map(|badge| badge.label())
                        .collect::<Vec<_>>()
                        .join(" · "),
                    Style::default().fg(Color::Red),
                )));
            }
            if let Some(patterns) = self.lane_claims().get(&lane.key) {
                header.push(TLine::from(Span::styled(
                    format!("claim {}", patterns.join(", ")),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
        // Context bar.
        if let Some(l) = lane {
            if let Some(used) = l.context_tokens {
                let window = self.loaded.config.medulla.context_window() as i64;
                let pct = ((used as f64 / window as f64) * 100.0).round().min(100.0) as i64;
                let filled = ((pct as f64 / 100.0) * 16.0).round() as usize;
                let bar = format!(
                    "{}{}",
                    "█".repeat(filled),
                    "░".repeat(16usize.saturating_sub(filled))
                );
                let c = if pct >= 90 {
                    Color::Red
                } else if pct >= 70 {
                    Color::Yellow
                } else {
                    Color::DarkGray
                };
                let detail = if l.role == AgentRole::Worker {
                    format!("{} tokens", fmt_tokens(used))
                } else {
                    format!("{}/{} ({pct}%)", fmt_tokens(used), fmt_tokens(window))
                };
                header.push(TLine::from(Span::styled(
                    format!("context {bar} {detail}"),
                    Style::default().fg(c),
                )));
            }
        }
        let capacity = (inner.height as usize).saturating_sub(header.len()).max(4);
        let max_scroll = content_lines.len().saturating_sub(capacity);
        let eff = self.agent_scroll.min(max_scroll);
        let end = content_lines.len() - eff;
        let view = &content_lines[end.saturating_sub(capacity)..end];
        let mut out = header;
        out.extend(view.iter().map(styled_to_tline));
        if eff > 0 {
            out.push(TLine::from(Span::styled(
                format!("↑ {eff} more line(s) below · k to catch up"),
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        f.render_widget(Paragraph::new(Text::from(out)), inner);
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
                    Span::styled(
                        task.review
                            .as_ref()
                            .map(|verdict| format!(" · {}", verdict.badge()))
                            .unwrap_or_default(),
                        style,
                    ),
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
                    Some(used) if item.role == AgentRole::Worker => {
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
                let badges = self
                    .lane_guard_report()
                    .badges(&item.key)
                    .into_iter()
                    .map(|badge| format!(" · {}", badge.label()))
                    .collect::<String>();
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
                let text = format!(
                    "{marker} {} · {}{ctx}{state}{sessions_note}{badges}",
                    item.label,
                    item.turns.len()
                );
                TLine::from(Span::styled(text, style))
            }
        }
    }

    /// The presence/status glyph for a lane row.
    pub(super) fn lane_marker(&self, item: &AgentLane, is_fn: bool) -> &'static str {
        if is_fn {
            "ƒ"
        } else if item.role != AgentRole::Worker {
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
        } else if item.role == AgentRole::Worker {
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
