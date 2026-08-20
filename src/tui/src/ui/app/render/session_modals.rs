//! The two overlays that own a harness handover: the picker that starts one,
//! and the question asked when the operator lets go of one.
//!
//! Both are centered popups over the content pane rather than strips under it,
//! because both are asked *about* something on screen — the picker names the
//! directory the rail is already showing, and the hand-back question is about
//! the pane immediately behind it.

use crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;

use super::super::types::{App, SessionPickerStep};

const HARNESS_TRAILER_LINES: usize = 3;

/// Lines drawn below the workspace list: the single "unmanaged · …" footer.
const WORKSPACE_TRAILER_LINES: usize = 1;

/// Width allotted to a workspace row's path (or favorite label and path)
/// before the dim provenance suffix is appended.
const WORKSPACE_ROW_WIDTH: usize = 43;

impl App {
    /// Draw the destructive confirmation for deleting the selected workflow.
    pub(super) fn draw_workflow_delete_prompt(&mut self, f: &mut Frame, area: Rect) {
        let Some((_, name)) = &self.workflow_delete_armed else {
            return;
        };
        let area = centered(area, 62, 9);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.theme.accent))
            .title(Span::styled(
                "Delete workflow",
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(area);
        f.render_widget(Clear, area);
        f.render_widget(block, area);
        f.render_widget(
            Paragraph::new(Text::from(vec![
                TLine::from(format!("Delete \"{name}\" permanently?")),
                TLine::from(Span::styled(
                    "Its definition and local workflow history will be removed.",
                    Style::default().fg(self.theme.dim_border),
                )),
                TLine::from(""),
                TLine::from("[Delete] remove workflow    [Esc] cancel"),
            ])),
            inner,
        );
    }

    /// Draw the confirmation that guards terminating a harness or dispatched task.
    ///
    /// The status line repeats the question for compact terminals, but the
    /// modal keeps the consequence in front of the operator while the content
    /// behind it still shows the session whose work will be lost.
    pub(super) fn draw_session_kill_prompt(&mut self, f: &mut Frame, area: Rect) {
        let Some((title, body)) = self.session_kill_prompt_copy() else {
            return;
        };
        let area = centered(area, 58, 8);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.theme.accent))
            .title(Span::styled(
                title,
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(area);
        f.render_widget(Clear, area);
        f.render_widget(block, area);
        f.render_widget(
            Paragraph::new(Text::from(vec![
                TLine::from(body),
                TLine::from(""),
                TLine::from("[Y] kill session    [any other key] cancel"),
            ])),
            inner,
        );
    }

    /// Return the copy for the active destructive session confirmation.
    fn session_kill_prompt_copy(&self) -> Option<(&'static str, &'static str)> {
        if self.harness_close_armed.is_some() {
            Some((
                "Kill this session?",
                "Its harness will stop and unsaved work will be lost.",
            ))
        } else {
            self.kill_armed.as_ref().map(|_| {
                (
                    "Kill this session?",
                    "The running task will be terminated and unsaved work will be lost.",
                )
            })
        }
    }

    /// Draw the "start a session" picker.
    pub(super) fn draw_harness_picker(&mut self, f: &mut Frame, area: Rect) {
        let Some(picker) = &self.session_picker else {
            return;
        };
        let (rows, title) = match picker.step {
            SessionPickerStep::Harness => (
                picker.choices.len(),
                "Choose a harness type — Enter workspace · Esc cancel",
            ),
            SessionPickerStep::Workspace => (
                picker.workspace_choices.len(),
                "Choose workspace — type to filter · Enter start · Esc back",
            ),
        };
        let height = (rows as u16).saturating_add(7).clamp(8, 18);
        let area = centered(area, 62, height);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.theme.accent))
            .title(Span::styled(
                title,
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(area);
        // Cleared first: this floats over the rail, and without it the rows
        // underneath show through the gaps between provider names.
        f.render_widget(Clear, area);
        f.render_widget(block, area);

        // Where each offered row lands, and which entry of the step's own list
        // it stands for. Recorded here rather than recomputed at click time
        // because the harness step *windows* a long list: the third row on
        // screen is not the third provider, and a pointer that assumed it was
        // would start the wrong CLI in the operator's workspace.
        let mut hits: Vec<(Rect, usize)> = Vec::new();
        // Rows are full-width, so aiming anywhere on the line works — a target
        // clipped to the label would make the short names hard to hit and leave
        // dead gaps between them.
        let row_hit = |index: usize, line: usize, hits: &mut Vec<(Rect, usize)>| {
            let y = inner.y + line as u16;
            if y < inner.bottom() {
                hits.push((
                    Rect {
                        x: inner.x,
                        y,
                        width: inner.width,
                        height: 1,
                    },
                    index,
                ));
            }
        };
        let mut lines = match picker.step {
            SessionPickerStep::Harness => {
                let capacity = (inner.height as usize).saturating_sub(HARNESS_TRAILER_LINES);
                let range = harness_choice_window(picker.choices.len(), picker.index, capacity);
                picker.choices[range.clone()]
                    .iter()
                    .enumerate()
                    .map(|(offset, choice)| {
                        let index = range.start + offset;
                        row_hit(index, offset, &mut hits);
                        let marker = if index == picker.index { "❯ " } else { "  " };
                        let style = if index == picker.index {
                            self.theme.selection()
                        } else {
                            Style::default()
                        };
                        TLine::from(Span::styled(
                            format!("{marker}{}", choice.display_name()),
                            style,
                        ))
                    })
                    .collect()
            }
            SessionPickerStep::Workspace => {
                let selected_session = picker
                    .choices
                    .get(picker.index)
                    .map(|choice| choice.display_name())
                    .unwrap_or("harness");
                let mut lines = vec![
                    TLine::from(Span::styled(
                        format!("  {selected_session}"),
                        Style::default().add_modifier(Modifier::BOLD),
                    )),
                    TLine::from(format!(
                        "  search › {}▌",
                        medulla::ui::util::clip_left(&picker.workspace_query, 46)
                    )),
                    TLine::from(""),
                ];
                if picker.workspace_choices.is_empty() {
                    lines.push(TLine::from(Span::styled(
                        "  No matching folders",
                        Style::default().add_modifier(Modifier::DIM),
                    )));
                }
                // The completions begin below the header lines already
                // pushed, so the offset is taken from the vector rather
                // than written as a constant that the next edit here would
                // silently make wrong.
                let first = lines.len();
                // Windowed on the selection, exactly as the harness step is.
                // Unwindowed, a workspace list longer than the popup drew its
                // first page and nothing else: the selection could move onto
                // a row that was never painted and never got a hit box, so
                // Enter started a session in a directory the operator could
                // neither see nor click.
                let capacity =
                    (inner.height as usize).saturating_sub(first + WORKSPACE_TRAILER_LINES);
                let range = harness_choice_window(
                    picker.workspace_choices.len(),
                    picker.workspace_index,
                    capacity,
                );
                lines.extend(
                    picker.workspace_choices[range.clone()]
                        .iter()
                        .enumerate()
                        .map(|(offset, choice)| {
                            let index = range.start + offset;
                            row_hit(index, first + offset, &mut hits);
                            let marker = if index == picker.workspace_index {
                                "❯ "
                            } else {
                                "  "
                            };
                            let style = if index == picker.workspace_index {
                                self.theme.selection()
                            } else {
                                Style::default()
                            };
                            // A bare path is clipped from the left, so the tail
                            // that identifies the directory survives. A named
                            // favorite instead clips from both ends: `★ name ·`
                            // is the distinguishing part the operator added, and
                            // clipping from the left would delete it whenever
                            // the path below runs long.
                            let display = match &choice.label {
                                Some(label) => medulla::ui::util::clip_middle(
                                    &format!("★ {label} · {}", choice.path),
                                    WORKSPACE_ROW_WIDTH,
                                ),
                                None => {
                                    medulla::ui::util::clip_left(&choice.path, WORKSPACE_ROW_WIDTH)
                                }
                            };
                            TLine::from(vec![
                                Span::styled(format!("{marker}{display}"), style),
                                Span::styled(
                                    format!("  {}", choice.source),
                                    Style::default().add_modifier(Modifier::DIM),
                                ),
                            ])
                        }),
                );
                lines
            }
        };
        self.hit_session_picker = Some((area, hits));
        let picker = self.session_picker.as_ref().expect("picker is present");
        if picker.step == SessionPickerStep::Harness {
            lines.push(TLine::from(""));
            lines.push(TLine::from(Span::styled(
                "  Next: choose a workspace",
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        // The one fact that makes this picker different from every other way
        // to start a session is that the session is unmanaged, so that is
        // stated on both steps. The keyboard verbs beside it are step-specific:
        // Tab complete and Shift+F save favorite need the workspace step's
        // text field and chosen directory, and advertising them on the harness
        // step — where neither key is bound — would send an operator pressing
        // the hint into silence.
        lines.push(TLine::from(Span::styled(
            harness_picker_hint(picker.step),
            Style::default().add_modifier(Modifier::DIM),
        )));
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }

    /// Build the question's answer line, recording where each answer landed.
    ///
    /// `row` is the answer line's offset from the top of `inner`. The segments
    /// are joined with the same separator the line has always used, and each one
    /// carrying a key becomes a click target spanning exactly its own label —
    /// including the `[Y]` bracket, because that is the part of "[Y] hand back"
    /// an operator aims at.
    ///
    /// A click replays the key rather than calling the answer directly, so the
    /// pointer and the keyboard cannot come to disagree about what `[N]` does.
    fn handback_answers(
        &mut self,
        inner: Rect,
        row: u16,
        segments: &[(&'static str, Option<KeyCode>)],
    ) -> TLine<'static> {
        const SEPARATOR: &str = " · ";
        self.hit_handback.clear();
        let y = inner.y + row;
        // A terminal too short for the grown modal clips the hint rather than
        // drawing it. Recording boxes for a row that was never painted would
        // answer the question for a click that landed on the pane behind.
        if y >= inner.bottom() {
            return TLine::from(
                segments
                    .iter()
                    .map(|(label, _)| *label)
                    .collect::<Vec<_>>()
                    .join(SEPARATOR),
            );
        }
        let mut x = inner.x;
        let mut spans = Vec::with_capacity(segments.len() * 2);
        for (index, (label, key)) in segments.iter().enumerate() {
            if index > 0 {
                spans.push(Span::raw(SEPARATOR));
                x = x.saturating_add(SEPARATOR.chars().count() as u16);
            }
            let width = label.chars().count() as u16;
            if let Some(key) = key {
                // Clipped to the pane: a narrow terminal truncates the line, and
                // a hit box reaching past the right edge would answer the
                // question for a click that landed on whatever is beside it.
                if x < inner.right() {
                    let rect = Rect {
                        x,
                        y,
                        width: width.min(inner.right() - x),
                        height: 1,
                    };
                    self.hit_handback.push((rect, *key));
                }
            }
            spans.push(Span::styled(
                *label,
                Style::default().add_modifier(Modifier::BOLD),
            ));
            x = x.saturating_add(width);
        }
        TLine::from(spans)
    }

    /// Draw the harness handover question, in whichever direction it is asked.
    pub(super) fn draw_handback_prompt(&mut self, f: &mut Frame, area: Rect) {
        let Some(prompt) = &self.handback_prompt else {
            return;
        };
        // The note is the one line here that grows without bound, so the box
        // grows with it. A fixed 12 rows clipped everything past the first row
        // of a draft — the keystrokes still landed, so an operator was typing
        // a handover note they could not read back or edit.
        //
        // Every body row is wrapped HERE, by the same helper that renders it,
        // so the hint's row is COUNTED rather than predicted. Dividing the
        // note's columns by the modal width got the count wrong the moment
        // ratatui word-wrapped: it broke after "Note: ", giving the label a row
        // of its own, so the hint sat below the hit boxes recorded for it and a
        // click answered nothing. Wrapping every line — not just the note —
        // keeps that true on a terminal narrow enough to wrap the prose too.
        const MODAL_WIDTH: u16 = 72;
        const BASE_HEIGHT: u16 = 12;
        /// Row of the answer hint when every body line fits on one row.
        const HINT_ROW: u16 = 6;
        // Measured against the width the modal will ACTUALLY get: `centered`
        // clamps to the terminal, so on a narrow one the text wraps into more
        // rows than 72 columns would suggest.
        let width = MODAL_WIDTH.min(area.width);
        let cols = usize::from(width.saturating_sub(2)).max(1);
        let dim = Style::default().fg(self.theme.dim_border);
        let accent = Style::default().fg(self.theme.accent);
        // Read out before `handback_answers` borrows `self` mutably.
        let is_takeover = prompt.is_takeover;
        let took_control = prompt.took_control;
        let editing_note = prompt.editing_note;
        let note_text = prompt.note.text.clone();

        // Empty for the takeover variant, which is fixed-size and answers at
        // its own row.
        let mut body: Vec<TLine<'static>> = Vec::new();
        if !is_takeover {
            // An operator who typed /takecontrol made a decision; one who simply
            // focused in may not know they are holding anything. The sentence
            // says which of the two happened rather than implying the second.
            let how = if took_control {
                "You took this session when you focused in."
            } else {
                "You asked for this session."
            };
            body.extend(wrapped(how, cols, None));
            body.extend(wrapped(
                "While you hold it, the orchestrator will not dispatch into it.",
                cols,
                None,
            ));
            body.push(TLine::from(""));
            // Said plainly, because it leaves the machine: the operator should
            // know what they are sending before they send it.
            body.extend(wrapped(
                "Handing back sends the orchestrator this pane's recent output.",
                cols,
                Some(dim),
            ));
            if note_text.is_empty() && !editing_note {
                body.extend(wrapped(
                    "Note: (none — press E to say what you were doing)",
                    cols,
                    Some(dim),
                ));
            } else {
                // The caret shows only while the note is being edited, so the
                // operator can tell at a glance whether `y` answers or types.
                let caret = if editing_note { "\u{2588}" } else { "" };
                let rows =
                    medulla::ui::util::wrap(&format!("{NOTE_LABEL}{note_text}{caret}"), cols);
                for (index, row) in rows.into_iter().enumerate() {
                    // The label keeps its accent, on whichever row it landed on.
                    if index == 0 {
                        let rest = row.strip_prefix(NOTE_LABEL).unwrap_or(&row).to_string();
                        body.push(TLine::from(vec![
                            Span::styled(NOTE_LABEL, accent),
                            Span::raw(rest),
                        ]));
                    } else {
                        body.push(TLine::from(row));
                    }
                }
            }
            body.push(TLine::from(""));
        }
        // The hit boxes are recorded against this, so a click follows the
        // visible controls down instead of answering from a fixed row 6.
        let hint_row = u16::try_from(body.len()).unwrap_or(HINT_ROW);
        let extra = hint_row.saturating_sub(HINT_ROW);
        let area = centered(area, width, BASE_HEIGHT.saturating_add(extra));
        let title = if prompt.is_takeover {
            "Take control of this session"
        } else {
            "You still have this session"
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.theme.accent))
            .title(Span::styled(
                title,
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(area);
        f.render_widget(Clear, area);
        f.render_widget(block, area);

        // Taking over is the short question: there is no note to send and
        // nothing to release, so the body says only what pressing Enter costs
        // the orchestrator.
        if prompt.is_takeover {
            let hint = self.handback_answers(
                inner,
                3,
                &[
                    ("[Enter] take control", Some(KeyCode::Enter)),
                    ("[Esc] cancel", Some(KeyCode::Esc)),
                ],
            );
            let lines = vec![
                TLine::from("The orchestrator is using this session."),
                TLine::from("Take control to type into it."),
                TLine::from(""),
                hint,
            ];
            f.render_widget(Paragraph::new(Text::from(lines)), inner);
            return;
        }

        let hint = if editing_note {
            self.handback_answers(
                inner,
                hint_row,
                &[
                    ("Type your note", None),
                    ("[Enter] hand back", Some(KeyCode::Enter)),
                    ("[Esc] back to the question", Some(KeyCode::Esc)),
                ],
            )
        } else {
            self.handback_answers(
                inner,
                hint_row,
                &[
                    ("[Y] hand back", Some(KeyCode::Char('y'))),
                    ("[E] add a note", Some(KeyCode::Char('e'))),
                    ("[N] keep it", Some(KeyCode::Char('n'))),
                    ("[Esc] stay here", Some(KeyCode::Esc)),
                ],
            )
        };
        let mut lines = body;
        lines.push(hint);
        // No `Wrap`: every line is already wrapped to `cols`, and letting the
        // widget wrap a second time is what moved the hint off its recorded row.
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }
}

/// The footer hint under the picker, stated per step.
///
/// `↑/↓ choose` applies to both steps, and the "unmanaged" statement is true
/// of any hand-started session, so both are said everywhere. `Tab complete`
/// and `Shift+F save favorite` are only meaningful on the workspace step — the
/// one with a text field to complete and a chosen directory to remember — and
/// the keys are not bound on the harness step, so the hint is not shown there.
pub(super) fn harness_picker_hint(step: SessionPickerStep) -> &'static str {
    match step {
        SessionPickerStep::Harness => "  ↑/↓ choose · unmanaged",
        SessionPickerStep::Workspace => {
            "  ↑/↓ choose · Tab complete · Shift+F save favorite · unmanaged"
        }
    }
}

/// Window a long harness list so the selected row always remains visible.
pub(super) fn harness_choice_window(
    total: usize,
    selected: usize,
    capacity: usize,
) -> std::ops::Range<usize> {
    let capacity = capacity.max(1).min(total);
    let selected = selected.min(total.saturating_sub(1));
    let start = selected
        .saturating_sub(capacity / 2)
        .min(total.saturating_sub(capacity));
    start..start + capacity
}

/// The note line's label. Shared with the height math above so the two cannot
/// disagree about how many columns the draft has left.
const NOTE_LABEL: &str = "Note: ";

/// A `width` × `height` box centered in `area`, clamped to fit.
///
/// Both overlays are small and fixed-size: a percentage would make the
/// three-line question fill half a large terminal.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

/// Wrap `text` to `cols` columns, one [`TLine`] per row.
///
/// Callers need the row COUNT as well as the rows, which `Paragraph`'s own
/// wrapping cannot give them: it wraps after the layout is already decided.
fn wrapped(text: &str, cols: usize, style: Option<Style>) -> Vec<TLine<'static>> {
    medulla::ui::util::wrap(text, cols)
        .into_iter()
        .map(|row| match style {
            Some(style) => TLine::from(Span::styled(row, style)),
            None => TLine::from(row),
        })
        .collect()
}
