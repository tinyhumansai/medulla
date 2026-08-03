//! Rendering for operator harness rows and agent-lane rows in the Agents rail.

use std::collections::HashSet;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span};
use unicode_width::UnicodeWidthChar;

use crate::ui::agents::{AgentLane, AgentRole, AgentRow, TaskStatus};
use crate::ui::util::fmt_tokens;
use crate::worker::pty::{HarnessAttention, HarnessControl, SessionRow, ATTENTION_GLYPH};

use super::super::super::super::types::App;
use super::super::super::color;
use super::harness_line;
use super::status::HarnessVisualState;
use super::wrap::{home_dir, wrap_line};
use super::CONT_INDENT;

/// Maximum terminal-cell width reserved for a session title in an agent label.
const SESSION_TITLE_MAX_CELLS: usize = 48;
/// Memory bound for zero-width Unicode sequences in a session title.
const SESSION_TITLE_MAX_CHARS: usize = 96;

impl App {
    /// Format one operator-started harness using the configured status-line layout.
    ///
    /// PTY attention overrides the ordinary state glyph and adds a textual cue,
    /// while the operator's field placement and visibility choices remain in
    /// force for the status line itself.
    pub(in crate::ui::app::render) fn own_harness_lines(
        &self,
        row: &SessionRow,
        active: bool,
        width: usize,
        now: i64,
    ) -> Vec<TLine<'static>> {
        let waiting = self.harness_attention(row);
        let alerting = waiting.is_some();
        let style = if waiting.is_some() {
            let mut attention = Style::default()
                .fg(self.theme.attention)
                .add_modifier(Modifier::BOLD);
            if self.theme.attention_blink {
                attention = attention.add_modifier(Modifier::SLOW_BLINK);
            }
            if active {
                attention.add_modifier(Modifier::REVERSED)
            } else {
                attention
            }
        } else if active {
            self.theme.selection()
        } else if row.control == HarnessControl::User {
            Style::default().fg(color("cyan"))
        } else {
            Style::default()
        };
        let glyph = match &waiting {
            Some(_) => ATTENTION_GLYPH,
            None => row.state.glyph(),
        };
        let detail_style = if active {
            style
        } else {
            style.add_modifier(Modifier::DIM)
        };
        let render = harness_line::HarnessLineStyle {
            active,
            width,
            alerting,
            state_glyph: glyph,
            primary: style,
            detail: detail_style,
        };
        let mut lines = harness_line::harness_lines(
            &self.loaded.config.status_line(),
            row,
            home_dir().as_deref(),
            render,
        );
        if let Some(cue) = waiting {
            let text = format!("  {ATTENTION_GLYPH} {}", cue.label(now));
            lines.extend(wrap_line(
                &TLine::from(Span::styled(text, style)),
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
                let needs_input = self.task_attention(&task.task_id, waiting_sessions);
                let mut style = if active {
                    self.theme.selection()
                } else {
                    Style::default()
                };
                if needs_input {
                    style = style.fg(self.theme.attention);
                    if self.theme.attention_blink {
                        style = style.add_modifier(Modifier::SLOW_BLINK);
                    }
                }
                let status_style = if active || needs_input {
                    style
                } else {
                    style.fg(color(task.status.color()))
                };
                let status = if needs_input {
                    HarnessVisualState::NeedsInput.label()
                } else {
                    task.status.label()
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
                    Span::styled(status.to_string(), status_style),
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
                let title_note = self
                    .lane_session_title(item)
                    .map(|title| format!(" · {title}"))
                    .unwrap_or_default();
                crate::ui::agent_lane::line(
                    marker,
                    format!("{}{title_note}", item.label),
                    format!(
                        " · {}{ctx}{state}{sessions_note}{work_note}",
                        item.turns.len()
                    ),
                    style,
                )
            }
        }
    }

    /// The title advertised by the live harness serving this lane's newest task.
    fn lane_session_title(&self, lane: &AgentLane) -> Option<String> {
        if !self.loaded.config.appearance.show_session_titles {
            return None;
        }
        let harnesses = self.harnesses.as_ref()?;
        running_session_title(lane, |task_id| {
            let id = harnesses.session_for_task(task_id)?;
            harnesses
                .sessions
                .row(&id)?
                .thread_name
                .map(|title| display_session_title(&title))
        })
    }
}

/// Flatten and bound an untrusted harness title before rail wrapping.
pub(super) fn display_session_title(title: &str) -> String {
    let mut displayed = String::new();
    let mut width = 0;
    for (index, character) in title.chars().enumerate() {
        if index >= SESSION_TITLE_MAX_CHARS {
            displayed.push('…');
            break;
        }
        let character = if character.is_control() {
            ' '
        } else {
            character
        };
        let character_width = character.width().unwrap_or(0);
        if width + character_width > SESSION_TITLE_MAX_CELLS.saturating_sub(1) {
            displayed.push('…');
            break;
        }
        displayed.push(character);
        width += character_width;
    }
    displayed
}

/// Resolve the newest running task whose harness has advertised a title.
pub(super) fn running_session_title(
    lane: &AgentLane,
    mut resolve: impl FnMut(&str) -> Option<String>,
) -> Option<String> {
    lane.tasks
        .iter()
        .filter(|task| task.status == TaskStatus::Running)
        .filter_map(|task| resolve(&task.task_id).map(|title| (task.last_at, title)))
        .max_by_key(|(last_at, _)| *last_at)
        .map(|(_, title)| title)
}
