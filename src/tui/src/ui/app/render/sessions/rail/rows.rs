//! Rendering for the session rows the Sessions rail draws.

use std::collections::HashSet;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::ui::util::{slug, SPINNER};
use crate::worker::pty::{
    AttentionKind, HarnessAttention, PtyState, SessionControl, SessionRow, ATTENTION_GLYPH,
};

use super::super::super::super::types::App;
use super::super::super::color;
use super::harness_line;
use super::wrap::{home_dir, wrap_line};
use super::CONT_INDENT;

/// Maximum terminal-cell width reserved for a session title in an agent label.
///
/// [`slug`] bounds its output in Unicode *characters*, which is the right unit
/// for a name but not for a row: 48 wide characters (`界`) still occupy 96
/// columns. The rail budgets cells, so the slug is clipped again here.
const SESSION_TITLE_MAX_CELLS: usize = 48;

/// What a row says when its harness is blocked on the operator.
const NEEDS_INPUT_LABEL: &str = "needs input";

impl App {
    /// Format one operator-started harness using the configured status-line layout.
    ///
    /// Lifecycle state chooses the glyph and colour, while the operator's field
    /// placement and visibility choices remain in force for the status line.
    pub(in crate::ui::app::render) fn own_session_lines(
        &self,
        row: &SessionRow,
        active: bool,
        width: usize,
        now: i64,
    ) -> Vec<TLine<'static>> {
        let cue = self.harness_attention(row, now);
        let failed = cue.as_ref().is_some_and(|cue| cue.kind.is_failure());
        let completed = cue
            .as_ref()
            .is_some_and(|cue| cue.kind == AttentionKind::Completed);
        let alerting = cue.is_some() && !completed;
        let style = if failed || (cue.is_some() && !completed) {
            let colour = if failed {
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
        } else if row.working {
            Style::default().fg(color("green"))
        } else if row.control == SessionControl::User {
            Style::default().fg(color("cyan"))
        } else {
            Style::default()
        };
        let glyph = match cue.as_ref() {
            Some(cue) if cue.kind.is_failure() => "✕".to_string(),
            Some(cue) if cue.kind == AttentionKind::Completed => "✓".to_string(),
            Some(_) => ATTENTION_GLYPH.to_string(),
            None if row.working => SPINNER[self.frame % SPINNER.len()].to_string(),
            None if matches!(row.state, PtyState::Exited { .. }) => "✓".to_string(),
            None => row.state.glyph().to_string(),
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
            state_glyph: glyph.clone(),
            primary: style,
            detail: detail_style,
        };
        let mut lines = harness_line::harness_lines(
            &self.loaded.config.status_line(),
            row,
            home_dir().as_deref(),
            render,
        );
        if let Some(cue) = cue {
            let text = format!("  {glyph} {}", cue.label(now));
            lines.extend(wrap_line(
                &TLine::from(Span::styled(text, style)),
                width,
                CONT_INDENT,
            ));
        }
        lines
    }

    /// The cue a harness row should draw, including lifecycle failures.
    pub(super) fn harness_attention(&self, row: &SessionRow, now: i64) -> Option<HarnessAttention> {
        if self.harness_focus.is_attached_to(&row.id) {
            return None;
        }
        crate::worker::pty::row_cue(row, now)
    }

    /// Format the `+N more` paging control for a lane the fold has paged.
    ///
    /// It highlights under the cursor like any other selectable row, because it
    /// is a control. Unselected it stays dim: it is a counter among real rows,
    /// and drawing it at full weight made a lane's tail read as another session.
    pub(super) fn overflow_line(&self, hidden: usize, active: bool) -> TLine<'static> {
        let label = if hidden > 0 {
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

    /// Format the row for a session the orchestrator dispatched.
    ///
    /// Such a session reaches the rail as a folded task rather than as a live
    /// pty — the two are one harness, but only the task side has a status and a
    /// work chip — so it is drawn from the task and not from
    /// [`own_session_lines`](Self::own_session_lines).
    pub(super) fn task_session_line(
        &self,
        task: &crate::ui::agents::TaskState,
        last: bool,
        active: bool,
        waiting_sessions: &HashSet<String>,
    ) -> TLine<'static> {
        let branch = if last { "└" } else { "├" };
        let needs_input = self.task_attention(&task.task_id, waiting_sessions);
        let mut style = if active {
            self.theme.selection()
        } else {
            Style::default()
        };
        if needs_input {
            style = self.theme.pulse(self.theme.attention, self.frame);
        }
        let status_style = if active || needs_input {
            style
        } else {
            style.fg(color(task.status.color()))
        };
        let status = if needs_input {
            // The one state the task's own status cannot express: the harness
            // has stopped on something only a person can answer, which the
            // backend never sees and so never reports.
            NEEDS_INPUT_LABEL
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
        // The harness's own window title, when it has advertised one and the
        // operator has asked to see them. It used to hang off the agent's lane
        // row, which resolved the newest running task's title; with the lane row
        // gone the row that *is* that task carries its own.
        let title = self
            .task_session_title(&task.task_id)
            .map(|title| format!(" · {title}"))
            .unwrap_or_default();
        TLine::from(vec![
            Span::styled(format!("   {branch} {} · ", task.task_id), style),
            Span::styled(status.to_string(), status_style),
            Span::styled(format!("{chip}{title}"), style),
        ])
    }

    /// The title advertised by the live harness serving `task_id`, if any.
    fn task_session_title(&self, task_id: &str) -> Option<String> {
        if !self.loaded.config.appearance.show_session_titles {
            return None;
        }
        let harnesses = self.local_sessions.as_ref()?;
        let id = harnesses.session_for_task(task_id)?;
        harnesses
            .sessions
            .row(&id)?
            .thread_name
            .and_then(|title| lane_title(&title))
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
