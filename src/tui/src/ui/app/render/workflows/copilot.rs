//! The copilot pane: the conversation that edits the graph beside it.
//!
//! A transcript above a composer, like the Agents tab — and now literally the
//! same widgets ([`crate::ui::chat`]) rather than a second implementation of the
//! idea. The difference is scope: this thread is about one workflow, and the
//! agent behind it holds the workflow tools rather than the whole machine.
//!
//! The transcript is anchored to the bottom. A copilot turn's useful part is its
//! end (the reply, and what changed), and a pane that kept the top would show
//! the instruction and scroll the answer away.
//!
//! The composer is a sibling of the transcript panel rather than nested inside
//! it, so it carries its own border. That border is the only thing that says
//! whether typing goes here — the Agents composer has always worked this way,
//! and the copilot reading differently made the two panes feel unrelated.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use medulla::ui::workflows::CopilotTurn;

use crate::ui::chat::{self, ComposerChrome};
use crate::ui::util::{clip, wrap, SPINNER};

use super::super::super::types::{App, WorkflowFocus};

/// Rows the composer may claim before the transcript starts losing more than it
/// can spare.
///
/// The shared widget grows with the draft on purpose; a pane this narrow cannot
/// honour that indefinitely, so a long instruction scrolls within the cap rather
/// than eating the conversation above it.
const MAX_COMPOSER_ROWS: u16 = 7;

impl App {
    /// Draw the copilot transcript and its composer.
    ///
    /// Only ever called while the copilot *is* the content pane
    /// ([`App::workflow_view`]), which is also the only state that gives it the
    /// keyboard — so there is no unfocused rendering of this pane to consider.
    pub(super) fn draw_workflow_copilot(&mut self, f: &mut Frame, area: Rect) {
        debug_assert_eq!(self.wf.focus, WorkflowFocus::Copilot);
        let busy = self.copilot_busy();

        let composer = chat::composer_height(&self.wf.draft.text).min(MAX_COMPOSER_ROWS);
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(composer)])
            .split(area);

        self.draw_copilot_transcript(f, rows[0], busy);
        chat::draw_composer(
            f,
            rows[1],
            &self.wf.draft,
            &self.theme,
            ComposerChrome {
                focused: true,
                busy,
                // Unlike the orchestrator's, this composer has no caption row
                // naming its target, so the placeholder is where it says what
                // asking here does.
                placeholder: Some(if self.wf.creating {
                    "Describe the workflow you want"
                } else {
                    "Ask for a change to this workflow"
                }),
            },
        );
    }

    /// The conversation so far, anchored to the bottom.
    fn draw_copilot_transcript(&mut self, f: &mut Frame, area: Rect, busy: bool) {
        // Named for what it is about, because the New row's thread is not about
        // any workflow yet and a bare "Copilot" would not say so.
        let subject = if self.wf.creating {
            " · new workflow"
        } else {
            ""
        };
        let title = if busy {
            format!("Copilot{subject} {}", SPINNER[self.frame % SPINNER.len()])
        } else {
            format!("Copilot{subject}")
        };
        let block = crate::ui::widgets::panel(&self.theme, title, true);
        let inner = block.inner(area);
        f.render_widget(block, area);
        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let width = inner.width as usize;
        // The hint occupies the last row, so the transcript gets what is left.
        // Drawn in every state, the intro one included: it is where `Esc` — the
        // way back to the graph — is written down, and the intro is exactly when
        // an operator has not found it yet.
        let body = Rect {
            height: inner.height.saturating_sub(1),
            ..inner
        };

        let mut lines: Vec<TLine> = Vec::new();
        if let Some(thread) = self.copilot() {
            for turn in &thread.turns {
                lines.extend(turn_lines(turn, width));
            }
        }

        let below = if lines.is_empty() {
            // Nothing said yet: the pane explains itself instead. Top-anchored,
            // unlike a transcript — this is a standing invitation rather than a
            // conversation whose end matters.
            let intro = if self.wf.creating {
                Intro::New
            } else if self.selected_workflow().is_some() {
                Intro::Edit
            } else {
                Intro::NothingSelected
            };
            f.render_widget(Paragraph::new(Text::from(intro_lines(intro, width))), body);
            self.wf.copilot_scroll = 0;
            0
        } else {
            let fit = chat::draw_transcript(f, body, lines, self.wf.copilot_scroll);
            // Written back so a held PageUp stops at the top instead of banking
            // presses that PageDown then has to spend before anything moves.
            self.wf.copilot_scroll = fit.scroll;
            fit.below
        };

        f.render_widget(
            Paragraph::new(TLine::from(Span::styled(
                clip(&hint(busy, below), width),
                Style::default().add_modifier(Modifier::DIM),
            ))),
            Rect {
                y: inner.y + inner.height.saturating_sub(1),
                height: 1,
                ..inner
            },
        );
    }
}

/// The one-line hint under the transcript.
///
/// Scrolled-back state wins over everything else: an operator who has walked up
/// the transcript needs to know there is more below before they need to be told
/// how to send.
fn hint(busy: bool, below: usize) -> String {
    if below > 0 {
        let plural = if below == 1 { "" } else { "s" };
        return format!("↓ {below} more line{plural} below");
    }
    if busy {
        // ⌃X really does stop it now: on this tab the chord reaches the
        // copilot's own host rather than the chat runtime. Enter still works —
        // it queues, and the queued line shows in the transcript.
        return "working… ⌃X stops it · ⏎ queues".to_string();
    }
    "⏎ send · Esc leave".to_string()
}

/// One transcript turn, wrapped and marked by role.
fn turn_lines(turn: &CopilotTurn, width: usize) -> Vec<TLine<'static>> {
    let mut style = Style::default().fg(super::super::color(turn.role.color()));
    if turn.role.dim() {
        style = style.add_modifier(Modifier::DIM);
    }
    // Wrapped like everything else. Tool lines used to be clipped to one row on
    // the grounds that their summary was already short — but they now carry
    // gate refusals and dry-run diagnostics, which name a node, a binding, and
    // the correction. Clipping those hides the half that says what to do.
    let text = turn.text.clone();
    let glyph = turn.role.glyph();
    wrap(&text, width.saturating_sub(2))
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

/// Which invitation the pane shows before its first instruction.
#[derive(Clone, Copy)]
enum Intro {
    /// The New row: nothing exists yet, and this conversation makes it.
    New,
    /// A workflow is selected and this conversation edits it.
    Edit,
    /// Neither — an empty catalogue with the cursor somewhere it cannot act.
    NothingSelected,
}

/// What the pane says before the first instruction.
fn intro_lines(intro: Intro, width: usize) -> Vec<TLine<'static>> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let (blurb, samples) = match intro {
        Intro::NothingSelected => {
            return vec![TLine::from(Span::styled("Select a workflow first.", dim))]
        }
        Intro::New => (
            "Describe what you want to happen and the copilot will build the \
             workflow — choosing the steps, wiring them up, and saving it once \
             it validates.",
            [
                "\"every morning, summarise new issues\"",
                "\"run the tests, then post the result to Slack\"",
                "\"when a PR opens, review it and comment\"",
            ],
        ),
        Intro::Edit => (
            "Ask for a change to this workflow and it will be made with the \
             same tools a coding agent uses — then validated and saved.",
            [
                "\"add a Slack step at the end\"",
                "\"why does the build step fail?\"",
                "\"split the review into two\"",
            ],
        ),
    };
    // Wrapped to the pane rather than hard-broken at the width it happened to
    // have when it was written, so a wider terminal does not show a ragged
    // column of short lines.
    let mut lines: Vec<TLine> = wrap(blurb, width)
        .into_iter()
        .map(|row| TLine::from(Span::styled(row, dim)))
        .collect();
    lines.push(TLine::from(""));
    for sample in samples {
        lines.push(TLine::from(Span::styled(clip(sample, width), dim)));
    }
    lines
}
