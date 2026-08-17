//! The two overlays that own a harness handover: the picker that starts one,
//! and the question asked when the operator lets go of one.
//!
//! Both are centered popups over the content pane rather than strips under it,
//! because both are asked *about* something on screen — the picker names the
//! directory the rail is already showing, and the hand-back question is about
//! the pane immediately behind it.

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
                    // The completions begin below the header lines already
                    // pushed, so the offset is taken from the vector rather
                    // than written as a constant that the next edit here would
                    // silently make wrong.
                    let first = lines.len();
                    lines.extend(picker.workspace_choices.iter().enumerate().map(
                        |(index, choice)| {
                            row_hit(index, first + index, &mut hits);
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
        self.hit_agent_picker = Some((area, hits));
        let picker = self.agent_picker.as_ref().expect("picker is present");
        if picker.step == AgentPickerStep::Harness {
            lines.push(TLine::from(""));
            lines.push(TLine::from(Span::styled(
                "  Next: choose a workspace",
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        lines.push(TLine::from(Span::styled(
            "  local session",
            Style::default().add_modifier(Modifier::DIM),
        )));
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
