//! Rendering for operator harness rows and agent-lane rows in the Agents rail.

use std::collections::HashSet;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::ui::agents::{AgentLane, AgentRole, AgentRow, TaskStatus};
use crate::ui::util::{fmt_tokens, slug};
use crate::worker::pty::{HarnessAttention, SessionControl, SessionRow};

use super::super::super::super::types::App;
use super::super::super::color;
use super::harness_line;
use super::status::{self, HarnessVisualState};
use super::wrap::{home_dir, wrap_line};
use super::CONT_INDENT;

/// Maximum terminal-cell width reserved for a session title in an agent label.
///
/// [`slug`] bounds its output in Unicode *characters*, which is the right unit
/// for a name but not for a row: 48 wide characters (`界`) still occupy 96
/// columns. The rail budgets cells, so the slug is clipped again here.
const SESSION_TITLE_MAX_CELLS: usize = 48;

impl App {
    /// Format one operator-started harness using the configured status-line layout.
    ///
    /// The row's lifecycle state chooses the glyph and the colour, and drives the
    /// two animations: a spinner while the harness is working, and a pulse while
    /// it is stuck. The operator's field placement and visibility choices remain
    /// in force for the status line itself.
    pub(in crate::ui::app::render) fn own_session_lines(
        &self,
        row: &SessionRow,
        active: bool,
        width: usize,
        now: i64,
    ) -> Vec<TLine<'static>> {
        let waiting = self.harness_attention(row, now);
        let state = status::classify_local(row, waiting.as_ref());
        // What `FieldVisibility::Alert` keys off. Any state the operator has to
        // act on counts, not just the screen-derived prompt it used to mean: a
        // harness that died should show its working directory for the same
        // reason one asking permission does.
        let alerting = state == HarnessVisualState::NeedsInput || state == HarnessVisualState::Errored;
        let style = if state.pulses() {
            // Errored pulses in red and needs-input in the configured attention
            // colour, so which of the two it is is legible before the wording is.
            let colour = if state == HarnessVisualState::Errored {
                color("red")
            } else {
                self.theme.attention
            };
            let pulse = self.theme.pulse(colour, self.frame);
            if active {
                pulse.add_modifier(Modifier::REVERSED)
            } else {
                pulse
            }
        } else if active {
            self.theme.selection()
        } else if state == HarnessVisualState::Working {
            Style::default().fg(state.color())
        } else if row.control == SessionControl::User {
            Style::default().fg(color("cyan"))
        } else {
            Style::default()
        };
        let glyph = state.glyph(self.frame);
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
            // The glyph on the cue line matches the row's, so a red `✕ codex
            // exited with 1` and a yellow `⚠ claude is asking permission` are
            // told apart at a glance rather than both reading as a warning.
            let text = format!("  {} {}", state.glyph(self.frame), cue.label(now));
            lines.extend(wrap_line(
                &TLine::from(Span::styled(text, style)),
                width,
                CONT_INDENT,
            ));
        }
        lines
    }

    /// The cue a harness row should draw, if it has one.
    ///
    /// Lifecycle cues (died, finished-and-retained) are added to the screen-derived
    /// one here rather than in the classifier, because only this layer knows
    /// which pane the operator is looking at — and a harness cannot be waiting on
    /// someone who is already sitting in front of it.
    pub(super) fn harness_attention(
        &self,
        row: &SessionRow,
        now: i64,
    ) -> Option<HarnessAttention> {
        if self.harness_focus.is_attached_to(&row.id) {
            return None;
        }
        crate::worker::pty::row_cue(row, now)
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
            // The overflow row is a control, so it highlights under the cursor
            // like any other selectable row. Unselected it stays dim: it is a
            // counter among real rows, and drawing it at full weight made a
            // lane's tail read as another task.
            AgentRow::More { hidden, .. } => {
                let label = if *hidden > 0 {
                    format!("   └ +{hidden} more")
                } else {
                    "   └ show less".to_string()
                };
                let style = if active {
                    self.theme.selection()
                } else {
                    Style::default().add_modifier(Modifier::DIM)
                };
                TLine::from(Span::styled(label, style))
            }
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
                    Span::styled(chip, style),
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
                    format!("{ctx}{state}{sessions_note}{work_note}"),
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
        let harnesses = self.local_sessions.as_ref()?;
        running_session_title(lane, |task_id| {
            let id = harnesses.session_for_task(task_id)?;
            harnesses
                .sessions
                .row(&id)?
                .thread_name
                .and_then(|title| lane_title(&title))
        })
    }
}

/// The rail label for a harness title, or `None` when the title carries no
/// name worth showing.
///
/// A title of pure punctuation or control bytes ("---") slugs to the empty
/// string. That is not a title: rendering it would leave a dangling ` · ` on
/// the row and, because the newest running task wins the lane, would hide an
/// older task that does have a meaningful title. The session-history label
/// path drops empty slugs the same way.
pub(super) fn lane_title(title: &str) -> Option<String> {
    let displayed = display_session_title(title);
    (!displayed.is_empty()).then_some(displayed)
}

/// Slug an untrusted harness title before rail wrapping.
///
/// The harness advertises a sentence ("Fix session handoff flow and pointer");
/// the rail has room for a name. [`slug`] keeps the first three meaningful
/// words and, because it treats every non-alphanumeric byte as a word break,
/// leaves no control byte, escape sequence, or newline to reach the pane.
///
/// The slug is then clipped to [`SESSION_TITLE_MAX_CELLS`], because its own
/// ceiling counts characters and a title of wide characters would otherwise
/// render twice as wide as the rail budgeted for it.
pub(super) fn display_session_title(title: &str) -> String {
    clip_cells(&slug(title), SESSION_TITLE_MAX_CELLS)
}

/// Clip `value` to `width` terminal cells, marking a cut with an ellipsis.
///
/// The ellipsis costs a cell of its own, so a clipped value keeps `width - 1`
/// cells of text. A character that straddles the budget is dropped whole rather
/// than split, which is what keeps the result a valid grapheme sequence.
fn clip_cells(value: &str, width: usize) -> String {
    if value.width() <= width {
        return value.to_string();
    }
    let budget = width.saturating_sub(1);
    let mut out = String::new();
    let mut used = 0;
    for character in value.chars() {
        let character_width = character.width().unwrap_or(0);
        if used + character_width > budget {
            break;
        }
        used += character_width;
        out.push(character);
    }
    out.push('…');
    out
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
