//! The Agents rail: the threads strip, and the list of lanes, fleet rows, and
//! agent templates beneath it.
//!
//! One cursor spans all of it (see [`rail`](crate::ui::app::rail)); this module
//! only draws what that cursor moves over, and formats each kind of row. The
//! per-lane glyph and status suffix live here too — they are the row's own
//! vocabulary, not the transcript's.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::ui::agents::{AgentLane, AgentRole, AgentRow, TaskStatus};
use crate::ui::util::fmt_tokens;
use crate::worker::pty::{HarnessControl, SessionRow};

use super::super::super::rail::RailRow;
use super::super::super::types::App;
use super::super::color;
use super::types::{AgentsPanes, Selection};

impl App {
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

        let running_tasks: usize = selection
            .lanes
            .iter()
            .map(|l| {
                l.tasks
                    .iter()
                    .filter(|t| t.status == TaskStatus::Running)
                    .count()
            })
            .sum();
        let agents = selection
            .lanes
            .iter()
            .filter(|l| !l.role.is_function())
            .count();
        let title = if running_tasks > 0 {
            format!("Agents · {agents} · {running_tasks} running")
        } else {
            format!("Agents · {agents}")
        };
        // The border says which half the keyboard is driving. Without it, Esc
        // moving focus to the rail is invisible until the next arrow press.
        let block = crate::ui::widgets::panel(&self.theme, title, self.agents_rail_focused());
        let inner = block.inner(panes.rail);
        f.render_widget(block, panes.rail);

        let capacity = (inner.height as usize).max(1);
        let start =
            crate::ui::selection::viewport_start(selection.active, selection.rows.len(), capacity);
        self.hit_agents = Some((inner, start));
        let lines: Vec<TLine> = selection
            .rows
            .iter()
            .skip(start)
            .take(capacity)
            .enumerate()
            .map(|(offset, row)| {
                self.rail_row_line(row, &selection.lanes, start + offset == selection.active)
            })
            .collect();
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
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

    /// Format one rail row: a lane row, or one of the operator's own harnesses.
    pub(super) fn rail_row_line(
        &self,
        row: &RailRow,
        lanes: &[AgentLane],
        active: bool,
    ) -> TLine<'static> {
        match row {
            RailRow::Agent(row) => self.agent_row_line(row, lanes, active),
            RailRow::HarnessSeparator => TLine::from(Span::styled(
                "── your harnesses ──",
                Style::default().add_modifier(Modifier::DIM),
            )),
            RailRow::Harness(row) => self.own_harness_line(row, active),
        }
    }

    /// Format one operator-started harness row.
    ///
    /// Says who holds it in words rather than only by colour: "unmanaged" is the
    /// whole reason the row exists, and an operator who hands one to the
    /// orchestrator needs to see that it took effect.
    fn own_harness_line(&self, row: &SessionRow, active: bool) -> TLine<'static> {
        let control = match row.control {
            HarnessControl::User => " · unmanaged",
            HarnessControl::Orchestrator => " · orchestrator",
        };
        // The directory is what distinguishes two harnesses of the same
        // provider, which is the common case once more than one is open.
        let where_ = short_path(&row.cwd);
        let text = format!(
            "   {} {} · {where_}{control}",
            row.state.glyph(),
            row.provider.as_str(),
        );
        let style = if active {
            self.theme.selection()
        } else if row.control == HarnessControl::User {
            Style::default().fg(color("cyan"))
        } else {
            Style::default()
        };
        TLine::from(Span::styled(text, style))
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
                // The chip is what makes a row worth reading at a glance: a
                // task that says "running · 12 turns" is indistinguishable from
                // every other one, while "3/7 ⑂2" says where it has got to.
                let chip = task
                    .work
                    .as_deref()
                    .map(crate::ui::work::work_chip)
                    .filter(|chip| !chip.is_empty())
                    .map(|chip| format!(" · {chip}"))
                    .unwrap_or_default();
                TLine::from(vec![
                    Span::styled(format!("   {branch} {} · ", task.task_id), style),
                    Span::styled(task.status.label().to_string(), status_style),
                    Span::styled(format!(" · {} turns{chip}", task.turns), style),
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
                let work_note = item
                    .work
                    .as_deref()
                    .map(crate::ui::work::work_chip)
                    .filter(|chip| !chip.is_empty())
                    .map(|chip| format!(" · {chip}"))
                    .unwrap_or_default();
                crate::ui::agent_lane::line(
                    marker,
                    item.label.clone(),
                    format!(
                        " · {}{ctx}{state}{sessions_note}{work_note}",
                        item.turns.len()
                    ),
                    style,
                )
            }
        }
    }

    /// The presence/status glyph for a lane row.
    pub(in crate::ui::app::render) fn lane_marker(
        &self,
        item: &AgentLane,
        is_fn: bool,
    ) -> &'static str {
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
    pub(in crate::ui::app::render) fn session_state(&self, item: &AgentLane) -> Option<String> {
        let (sid, pid) = (item.session_id.as_ref()?, item.parent_agent_id.as_ref()?);
        self.snapshot
            .sessions
            .get(pid)?
            .iter()
            .find(|s| &s.id == sid)
            .map(|s| s.state.clone())
    }

    /// A short human-readable state suffix for a lane row.
    pub(in crate::ui::app::render) fn lane_state(&self, item: &AgentLane) -> String {
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

/// The tail of a path, for a rail row that has one column to spare.
///
/// A harness's working directory is usually a long absolute path whose last two
/// segments are the only part that distinguishes it from the next one. Showing
/// the head instead would give every row the same prefix and no information.
fn short_path(path: &str) -> String {
    let parts: Vec<&str> = path.trim_end_matches('/').rsplit('/').take(2).collect();
    match parts.len() {
        0 => "/".to_string(),
        1 => parts[0].to_string(),
        _ => format!("{}/{}", parts[1], parts[0]),
    }
}
