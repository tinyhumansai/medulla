//! Rendering for operator harness rows and agent-lane rows in the Agents rail.

use std::collections::HashSet;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span};
use unicode_width::UnicodeWidthStr;

use crate::ui::agents::{AgentLane, AgentRole, AgentRow};
use crate::ui::util::{clip, fmt_tokens};
use crate::worker::pty::{HarnessAttention, HarnessControl, SessionRow, ATTENTION_GLYPH};

use super::super::super::super::types::App;
use super::super::super::color;
use super::status::HarnessVisualState;
use super::wrap::{home_dir, short_home, wrap_line, wrap_path};
use super::CONT_INDENT;

impl App {
    /// Format one operator-started harness as one compact rail row.
    pub(super) fn own_harness_lines(
        &self,
        row: &SessionRow,
        active: bool,
        width: usize,
        now: i64,
    ) -> Vec<TLine<'static>> {
        let waiting = self.harness_attention(row);
        let control = match row.control {
            HarnessControl::User => " · unmanaged",
            HarnessControl::Orchestrator => " · orchestrator",
        };
        let style = if active {
            self.theme.selection()
        } else if waiting.is_some() {
            Style::default()
                .fg(HarnessVisualState::NeedsInput.color())
                .add_modifier(Modifier::BOLD | Modifier::SLOW_BLINK)
        } else if row.control == HarnessControl::User {
            Style::default().fg(color("cyan"))
        } else {
            Style::default()
        };
        let glyph = match &waiting {
            Some(_) => ATTENTION_GLYPH,
            None => row.state.glyph(),
        };
        let head = format!("{glyph} {}{control}", row.provider.as_str());
        let head = if width == 0 {
            String::new()
        } else {
            clip(&head, width)
        };
        let detail_style = if active {
            style
        } else {
            style.add_modifier(Modifier::DIM)
        };
        const SEPARATOR: &str = " · ";
        let mut spans = vec![Span::styled(head, style)];
        let mut used = spans[0].width();
        let appearance = &self.loaded.config.appearance;
        if appearance.show_harness_branch {
            if let Some(branch) = row.branch.as_deref() {
                let remaining = width.saturating_sub(used + SEPARATOR.width());
                let reserve_for_path = if appearance.show_harness_path { 7 } else { 0 };
                let branch_room = remaining.saturating_sub(reserve_for_path).min(16);
                if branch_room >= 2 {
                    let branch = clip(branch, branch_room);
                    used += SEPARATOR.width() + branch.width();
                    spans.push(Span::styled(format!("{SEPARATOR}{branch}"), detail_style));
                }
            }
        }
        let path_room = width.saturating_sub(used + SEPARATOR.width());
        if appearance.show_harness_path && path_room >= 4 {
            let path = short_home(&row.cwd, home_dir().as_deref());
            let path = wrap_path(&path, path_room, 1).concat();
            spans.push(Span::styled(format!("{SEPARATOR}{path}"), detail_style));
        }
        let mut lines = vec![TLine::from(spans)];
        if let Some(cue) = waiting {
            let text = format!("  {ATTENTION_GLYPH} {}", cue.label(now));
            lines.extend(wrap_line(
                &TLine::from(Span::styled(clip(&text, width.max(1)), style)),
                width,
                CONT_INDENT,
            ));
        }
        lines
    }

    /// The cue a harness row should blink about, if it should.
    pub(super) fn harness_attention(&self, row: &SessionRow) -> Option<HarnessAttention> {
        if !row.state.is_running() || self.harness_focus.is_attached_to(&row.id) {
            return None;
        }
        row.attention.clone()
    }

    /// Format one Agents-list row (separator, "more", sub-task, or lane).
    pub(super) fn agent_row_line(
        &self,
        row: &AgentRow,
        lanes: &[AgentLane],
        active: bool,
        waiting_sessions: &HashSet<String>,
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
                let style = if active {
                    self.theme.selection()
                } else {
                    Style::default()
                };
                let status_style = if active {
                    style
                } else {
                    style.fg(color(task.status.color()))
                };
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
                let state = self.lane_state(item, waiting_sessions);
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
                let style = self.lane_style(item, is_fn, active, waiting_sessions);
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
}
