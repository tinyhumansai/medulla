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

use super::super::types::{AgentPickerStep, App};

const HARNESS_TRAILER_LINES: usize = 3;

impl App {
    /// Draw the "start a session" picker.
    pub(super) fn draw_harness_picker(&mut self, f: &mut Frame, area: Rect) {
        let Some(picker) = &self.agent_picker else {
            return;
        };
        let (rows, title) = match picker.step {
            AgentPickerStep::Harness => (
                picker.choices.len(),
                "Choose a harness type — ↑/↓ · Enter workspace · Esc cancel",
            ),
            AgentPickerStep::Decision => (2, "Choose control — ↑/↓ · Enter confirm · Esc back"),
            AgentPickerStep::Workspace => (
                picker.workspace_choices.len(),
                "Choose workspace — type to filter · Tab complete · Enter start · Esc back",
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

        let mut lines =
            match picker.step {
                AgentPickerStep::Harness => {
                    let capacity = (inner.height as usize).saturating_sub(HARNESS_TRAILER_LINES);
                    let range = harness_choice_window(picker.choices.len(), picker.index, capacity);
                    picker.choices[range.clone()]
                        .iter()
                        .enumerate()
                        .map(|(offset, choice)| {
                            let index = range.start + offset;
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
                AgentPickerStep::Decision => {
                    let selected = picker
                        .choices
                        .get(picker.index)
                        .map(|choice| choice.display_name())
                        .unwrap_or("harness");
                    let managed = picker.managed;
                    // The workspace was chosen on the previous step, so name it
                    // here: this is the last screen before the harness starts,
                    // and "in which directory" is the fact the operator has to
                    // be sure of before answering.
                    let ws_display = picker
                        .workspace_choices
                        .get(picker.workspace_index)
                        .map(|choice| choice.path.as_str())
                        .unwrap_or("(none)");
                    vec![
                        TLine::from(Span::styled(
                            format!("  {selected}"),
                            Style::default().add_modifier(Modifier::BOLD),
                        )),
                        TLine::from(Span::styled(
                            // Clipped from the left like the workspace list
                            // itself: the leaf directory is what identifies a
                            // path, and it is the end that a plain truncation
                            // throws away.
                            format!("  in {}", medulla::ui::util::clip_left(ws_display, 52)),
                            Style::default().add_modifier(Modifier::DIM),
                        )),
                        TLine::from(""),
                        TLine::from(Span::styled(
                            format!(
                                "{}Managed · orchestrator can dispatch into it",
                                if managed { "❯ " } else { "  " }
                            ),
                            if managed {
                                self.theme.selection()
                            } else {
                                Style::default()
                            },
                        )),
                        TLine::from(Span::styled(
                            format!(
                                "{}Unmanaged · you hold it, orchestrator won't dispatch",
                                if !managed { "❯ " } else { "  " }
                            ),
                            if !managed {
                                self.theme.selection()
                            } else {
                                Style::default()
                            },
                        )),
                        TLine::from(""),
                        TLine::from(Span::styled(
                            "  Enter confirm · Esc back",
                            Style::default().add_modifier(Modifier::DIM),
                        )),
                    ]
                }
                AgentPickerStep::Workspace => {
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
                    lines.extend(picker.workspace_choices.iter().enumerate().map(
                        |(index, choice)| {
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
                            TLine::from(vec![
                                Span::styled(
                                    format!(
                                        "{marker}{}",
                                        medulla::ui::util::clip_left(&choice.path, 43)
                                    ),
                                    style,
                                ),
                                Span::styled(
                                    format!("  {}", choice.source),
                                    Style::default().add_modifier(Modifier::DIM),
                                ),
                            ])
                        },
                    ));
                    lines
                }
            };
        if picker.step == AgentPickerStep::Harness {
            lines.push(TLine::from(""));
            lines.push(TLine::from(Span::styled(
                "  Next: choose a workspace",
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        // Said here as well as in the status line, because it is the one fact
        // that makes this different from every other way to start a harness.
        // Skip on the Decision step — it already shows both options inline.
        if picker.step != AgentPickerStep::Decision {
            lines.push(TLine::from(Span::styled(
                "  unmanaged · the orchestrator will not dispatch into it",
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
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
        let area = centered(area, 72, 12);
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

        // An operator who typed /takecontrol made a decision; one who simply
        // focused in may not know they are holding anything. The sentence says
        // which of the two happened rather than implying the second.
        let how = if prompt.took_control {
            "You took this session when you focused in."
        } else {
            "You asked for this session."
        };
        // The note line shows a caret only while it is being edited, so the
        // operator can tell at a glance whether `y` will answer or type.
        let note = if prompt.editing_note {
            TLine::from(vec![
                Span::styled("Note: ", Style::default().fg(self.theme.accent)),
                Span::raw(prompt.note.text.clone()),
                Span::styled("█", Style::default().fg(self.theme.accent)),
            ])
        } else if prompt.note.text.is_empty() {
            TLine::from(Span::styled(
                "Note: (none — press E to say what you were doing)",
                Style::default().fg(self.theme.dim_border),
            ))
        } else {
            TLine::from(format!("Note: {}", prompt.note.text))
        };
        let editing_note = prompt.editing_note;
        let hint = if editing_note {
            self.handback_answers(
                inner,
                6,
                &[
                    ("Type your note", None),
                    ("[Enter] hand back", Some(KeyCode::Enter)),
                    ("[Esc] back to the question", Some(KeyCode::Esc)),
                ],
            )
        } else {
            self.handback_answers(
                inner,
                6,
                &[
                    ("[Y] hand back", Some(KeyCode::Char('y'))),
                    ("[E] add a note", Some(KeyCode::Char('e'))),
                    ("[N] keep it", Some(KeyCode::Char('n'))),
                    ("[Esc] stay here", Some(KeyCode::Esc)),
                ],
            )
        };
        let lines = vec![
            TLine::from(how),
            TLine::from("While you hold it, the orchestrator will not dispatch into it."),
            TLine::from(""),
            // Said plainly, because it leaves the machine: the operator should
            // know what they are sending before they send it.
            TLine::from(Span::styled(
                "Handing back sends the orchestrator this pane's recent output.",
                Style::default().fg(self.theme.dim_border),
            )),
            note,
            TLine::from(""),
            hint,
        ];
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
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
