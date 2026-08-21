//! Session overlays: the launcher, workflow-delete prompt, and session-close prompt.
//!
//! All are centered popups over the content pane rather than strips under it:
//! the picker names the directory the rail is already showing, while the close
//! prompt keeps the session whose work would be lost visible behind it.

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
                "Choose a session type — Enter workspace · Esc cancel",
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
