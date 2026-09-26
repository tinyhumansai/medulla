//! Drawing the graph as an outline.
//!
//! One step per row, in the order the work happens, with a gate's arms indented
//! beneath it. Nothing is clipped to a column width and nothing folds, so the
//! only thing a wide pane buys is fewer wrapped lines — which is what makes this
//! the readable view of a long chain on a narrow terminal.
//!
//! The row order and the duplicate-step grouping are the model's
//! ([`crate::ui::app::workflows`]); this module turns them into lines.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use medulla::ui::workflows::RunOverlay;

use super::super::super::types::{App, WorkflowFocus};
use super::super::super::workflows::OutlineRow;

/// Columns one branch level indents by.
///
/// Two: enough that a nested arm reads as nested, small enough that a gate three
/// deep has not eaten the row it is trying to show.
const INDENT: usize = 2;

impl App {
    /// Draw the selected workflow's graph as an outline.
    pub(in crate::ui::app::render) fn draw_workflow_outline(&mut self, f: &mut Frame, area: Rect) {
        let focused = matches!(self.wf.focus, WorkflowFocus::Canvas);
        let block = crate::ui::widgets::panel(&self.theme, self.outline_title(), focused);
        let inner = block.inner(area);
        self.wf.graph_rows = inner.height as usize;
        self.scroll_outline_to_cursor();
        f.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let rows = self.outline_rows();
        if rows.is_empty() {
            f.render_widget(Paragraph::new(Text::from(self.empty_canvas_lines())), inner);
            return;
        }

        let overlay = self.selected_workflow_run().map(RunOverlay::new);
        let groups = self.duplicate_sinks();
        let cursor = self.outline_cursor_row(&rows);
        let mut lines: Vec<Line<'static>> = rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                self.outline_line(
                    row,
                    inner.width as usize,
                    index == cursor,
                    overlay.as_ref(),
                    &groups,
                )
            })
            .collect();
        // The duplication the rows only badge, spelled out once under them.
        for note in self.duplicate_sink_notes() {
            lines.push(Line::from(Span::styled(
                crate::ui::util::clip(&note, inner.width as usize),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::DIM),
            )));
        }
        let top = self.wf.canvas_row.min(lines.len().saturating_sub(1));
        f.render_widget(Paragraph::new(Text::from(lines[top..].to_vec())), inner);
    }

    /// The outline panel's title. The canvas's, plus the way back to the wires.
    fn outline_title(&self) -> String {
        format!("{} · outline · v wires", self.workflow_title())
    }

    /// One row: its branch prefix, its marker, and its name.
    fn outline_line(
        &self,
        row: &OutlineRow,
        width: usize,
        cursor: bool,
        overlay: Option<&RunOverlay>,
        groups: &std::collections::HashMap<String, super::super::super::workflows::SinkGroup>,
    ) -> Line<'static> {
        let Some(node) = self.workflow_layout().node(row.node) else {
            return Line::from("");
        };
        let dim = Style::default().add_modifier(Modifier::DIM);
        let mut spans = vec![Span::styled(" ".repeat(row.depth * INDENT), dim)];
        // The arm's own name is the point of the row: which choice leads here.
        if let Some(arm) = &row.arm {
            spans.push(Span::styled(
                format!("{} {arm} → ", if row.last_arm { "╰" } else { "├" }),
                dim,
            ));
        }

        let run = overlay.map(|overlay| overlay.node(&node.id));
        let color = match &run {
            Some(state) => super::super::color(state.state.color()),
            None => super::super::color(medulla::ui::workflows::graph::color_for_kind(&node.kind)),
        };
        let mut style = Style::default().fg(color).add_modifier(Modifier::BOLD);
        if cursor {
            style = style.add_modifier(Modifier::REVERSED);
        }
        if matches!(
            run.as_ref().map(|state| state.state),
            Some(medulla::ui::workflows::NodeRunState::Pending)
        ) {
            style = style.add_modifier(Modifier::DIM);
        }

        // A step that is one of several identical siblings is drawn as the one
        // step it is, under the name they agree on: four rows reading
        // `Final report ×4` say what four differently-named rows could not.
        let name = match groups.get(&node.id) {
            Some(group) => format!("{} ×{}", group.label, group.size),
            None => node.name.clone(),
        };
        if row.revisit {
            // A second arrival is a reference to the row that drew it, not a
            // second copy of the step and everything under it.
            spans.push(Span::styled(format!("↑ {} {name}", node.glyph), dim));
            return Line::from(spans);
        }
        let mark = run
            .as_ref()
            .map(|state| format!("{} ", state.state.glyph()))
            .unwrap_or_default();
        spans.push(Span::styled(format!("{mark}{} {name}", node.glyph), style));
        // The summary is what the canvas has no room for at all: at outline
        // width there is usually room for the one line that says what the step
        // is configured to do. Clipped to what is left of the row, and dropped
        // entirely when it is an expression — a jq program spilling across the
        // pane is the clutter this view exists to remove, and the preview under
        // it decodes the same program into prose.
        let summary = node.summary.trim();
        if !summary.is_empty() && !summary.starts_with('=') && !summary.contains("{{") {
            let used: usize = spans.iter().map(|span| span.content.chars().count()).sum();
            let room = width.saturating_sub(used + 2);
            if room >= 8 {
                spans.push(Span::styled(
                    format!("  {}", crate::ui::util::clip(summary, room)),
                    dim,
                ));
            }
        }
        Line::from(spans)
    }

    /// The notes under the outline: one pair of lines per group of duplicated
    /// terminal steps.
    ///
    /// Two lines rather than one because the important half — that these are one
    /// step, and what separates the copies — must survive a narrow pane, and a
    /// single line long enough to also list four names does not.
    fn duplicate_sink_notes(&self) -> Vec<String> {
        let mut groups: Vec<super::super::super::workflows::SinkGroup> =
            self.duplicate_sinks().into_values().collect();
        groups.sort_by(|a, b| a.label.cmp(&b.label));
        groups.dedup_by(|a, b| a.label == b.label && a.names == b.names);
        groups
            .into_iter()
            .flat_map(|group| {
                let differs = if group.differs.is_empty() {
                    "nothing".to_string()
                } else {
                    group.differs.join(", ")
                };
                [
                    format!(
                        "≡ {} ×{} · one step under {} names · differs only in {differs}",
                        group.label, group.size, group.size
                    ),
                    format!("↳ {}", group.names.join(" · ")),
                ]
            })
            .collect()
    }
}
