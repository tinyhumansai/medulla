//! The copilot pane: the conversation that edits the graph beside it.
//!
//! A transcript above a composer, like the Agents tab — deliberately, because it
//! is the same gesture. The difference is scope: this thread is about one
//! workflow, and the agent behind it holds the workflow tools rather than the
//! whole machine.
//!
//! The transcript is anchored to the bottom. A copilot turn's useful part is its
//! end (the reply, and what changed), and a pane that kept the top would show
//! the instruction and scroll the answer away.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use medulla::ui::workflows::CopilotTurn;

use crate::ui::util::{wrap, SPINNER};

use super::super::super::types::{App, WorkflowFocus};

impl App {
    /// Draw the copilot transcript and its composer.
    pub(super) fn draw_workflow_copilot(&mut self, f: &mut Frame, area: Rect) {
        let focused = self.wf.focus == WorkflowFocus::Copilot;
        let busy = self.copilot_busy();
        let title = if busy {
            format!("Copilot {}", SPINNER[self.frame % SPINNER.len()])
        } else {
            "Copilot".to_string()
        };
        let block = crate::ui::widgets::panel(&self.theme, title, focused);
        let inner = block.inner(area);
        f.render_widget(block, area);
        if inner.height < 2 || inner.width == 0 {
            return;
        }

        // The composer takes as many rows as the draft has lines, so a long
        // instruction is visible while it is being written.
        let draft_rows = (self.wf.draft.text.split('\n').count() as u16).clamp(1, 5);
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(draft_rows + 1)])
            .split(inner);

        self.draw_copilot_transcript(f, rows[0]);
        self.draw_copilot_composer(f, rows[1], focused, busy);
    }

    /// The conversation so far, anchored to the bottom.
    fn draw_copilot_transcript(&self, f: &mut Frame, area: Rect) {
        let width = area.width as usize;
        let dim = Style::default().add_modifier(Modifier::DIM);
        let Some(thread) = self.copilot() else {
            f.render_widget(
                Paragraph::new(Text::from(intro_lines(self.selected_workflow().is_some()))),
                area,
            );
            return;
        };
        if thread.turns.is_empty() {
            f.render_widget(Paragraph::new(Text::from(intro_lines(true))), area);
            return;
        }

        let mut lines: Vec<TLine> = Vec::new();
        for turn in &thread.turns {
            lines.extend(turn_lines(turn, width));
        }
        // Anchored to the bottom, minus whatever the operator has scrolled back
        // by. A scroll past the top stops at the top rather than emptying the
        // pane.
        let height = area.height as usize;
        let overflow = lines.len().saturating_sub(height);
        let start = overflow.saturating_sub(self.wf.copilot_scroll);
        let visible: Vec<TLine> = lines.into_iter().skip(start).take(height).collect();
        let _ = dim;
        f.render_widget(Paragraph::new(Text::from(visible)), area);
    }

    /// The instruction composer, and what it is waiting on.
    fn draw_copilot_composer(&self, f: &mut Frame, area: Rect, focused: bool, busy: bool) {
        let dim = Style::default().add_modifier(Modifier::DIM);
        let hint = if busy {
            "working — ⌃X aborts"
        } else if focused {
            "⏎ send · Esc leave"
        } else {
            "Tab to type"
        };
        let mut lines = vec![TLine::from(Span::styled(hint, dim))];
        if busy {
            lines.push(TLine::from(Span::styled(
                self.wf.draft.text.clone(),
                Style::default().add_modifier(Modifier::DIM),
            )));
        } else if focused {
            lines.push(crate::ui::widgets::prompt_line(&self.wf.draft, &self.theme));
        } else {
            lines.push(TLine::from(Span::styled(
                if self.wf.draft.text.is_empty() {
                    "Ask for a change to this workflow".to_string()
                } else {
                    self.wf.draft.text.clone()
                },
                dim,
            )));
        }
        f.render_widget(Paragraph::new(Text::from(lines)), area);
    }
}

/// One transcript turn, wrapped and marked by role.
fn turn_lines(turn: &CopilotTurn, width: usize) -> Vec<TLine<'static>> {
    let mut style = Style::default().fg(super::super::color(turn.role.color()));
    if turn.role.dim() {
        style = style.add_modifier(Modifier::DIM);
    }
    let glyph = turn.role.glyph();
    wrap(&turn.text, width.saturating_sub(2))
        .into_iter()
        .enumerate()
        .map(|(index, row)| {
            let lead = if index == 0 {
                format!("{glyph} ")
            } else {
                "  ".to_string()
            };
            TLine::from(Span::styled(format!("{lead}{row}"), style))
        })
        .collect()
}

/// What the pane says before the first instruction.
fn intro_lines(has_workflow: bool) -> Vec<TLine<'static>> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let body = if has_workflow {
        vec![
            "Ask for a change to this",
            "workflow and it will be made",
            "with the same tools a coding",
            "agent uses — then validated",
            "and saved.",
            "",
            "\"add a Slack step at the end\"",
            "\"why does the build step fail?\"",
            "\"split the review into two\"",
        ]
    } else {
        vec!["Select a workflow first."]
    };
    body.into_iter()
        .map(|line| TLine::from(Span::styled(line, dim)))
        .collect()
}
