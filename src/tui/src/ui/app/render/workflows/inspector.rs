//! The node inspector: what the graph cursor is on, in full.
//!
//! The *same* reading of the node as the peek under the graph — one node view,
//! not two. There used to be two, and neither could be read: the peek rendered
//! a script with syntax highlighting and clipped every line at the pane edge,
//! while this screen re-derived the declaration as a `field  value` table and
//! clipped it again, so a ninety-line script and a six-thousand-character prompt
//! both came out as a column of `…`. This screen is now the peek with the whole
//! pane to itself, scrollable, plus the two things it has room for that the peek
//! does not: the run in full, and what the workflow has learned.
//!
//! `i` opens and closes it; the graph is what it closes back to.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use medulla::ui::workflows::{note_rows, proposal_detail, run_overview};

use crate::ui::util::clip;

use super::super::super::types::App;

impl App {
    /// Draw the node inspector over the canvas.
    pub(super) fn draw_workflow_inspector(&mut self, f: &mut Frame, area: Rect) {
        // The hint lives in the title so every content row belongs to the node.
        let title = self
            .workflow_preview_title(" · i back to the graph")
            .unwrap_or_else(|| "Node · i back to the graph".to_string());
        // Focused: the inspector only exists while it is the thing being looked
        // at, so a dim border would say it was somewhere else on screen.
        let block = crate::ui::widgets::panel(&self.theme, title, true);
        let inner = block.inner(area);
        f.render_widget(block, area);
        if inner.height == 0 || inner.width == 0 {
            return;
        }
        let width = inner.width as usize;
        let dim = Style::default().add_modifier(Modifier::DIM);

        if self.selected_graph_node().is_none() {
            f.render_widget(
                Paragraph::new(TLine::from(Span::styled(
                    "No node selected. Tab to the canvas and use the arrows.",
                    dim,
                ))),
                inner,
            );
            return;
        }

        let mut lines: Vec<TLine> = Vec::new();
        // The run in full, above the step. “What was this run” — its inputs, who
        // started it, how long it took, what its diagnosis found — has no other
        // exhaustive view: the rail label carries a status and a step count, and
        // every other field on the record is reachable only through the CLI.
        if let Some(run) = self.selected_workflow_run() {
            lines.push(TLine::from(Span::styled("  Run", dim)));
            for row in run_overview(run) {
                lines.push(TLine::from(vec![
                    Span::styled(format!("{:>16}  ", row.label), dim),
                    Span::raw(clip(&row.value, width.saturating_sub(18))),
                ]));
            }
            lines.push(TLine::from(""));
            lines.push(TLine::from(Span::styled("  Step", dim)));
        }

        // The step itself, rendered exactly as the peek renders it — a script as
        // a script, a prompt as prose — with the whole pane to wrap into rather
        // than a right edge to be clipped at.
        lines.extend(self.workflow_preview_lines(width));
        self.push_learned(&mut lines, width, dim);

        // Scrolled from the top: a long config is read downward, and the
        // interesting fields (id, kind, name) are at the top of it. The offset
        // is the preview's, so PageUp/PageDown mean the same thing in both.
        let visible = inner.height as usize;
        let visual_lines = lines
            .iter()
            .map(|line| line.width().max(1).div_ceil(width))
            .sum::<usize>();
        self.wf.preview_scroll = self
            .wf
            .preview_scroll
            .min(visual_lines.saturating_sub(visible));
        f.render_widget(
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: false })
                .scroll((self.wf.preview_scroll.min(u16::MAX as usize) as u16, 0)),
            inner,
        );
    }
}

impl App {
    /// Append what this workflow has learned, below the node's declaration.
    ///
    /// Below rather than above: the node under the cursor is what the operator
    /// is looking at, and the journal is context for it. A pending proposal
    /// comes first within the section, because it is the only part waiting on
    /// them.
    fn push_learned(&self, lines: &mut Vec<TLine<'static>>, width: usize, dim: Style) {
        let notes = self.workflow_notes();
        let proposal = self.visible_proposal();
        if notes.is_empty() && proposal.is_none() {
            return;
        }
        let value_width = width.saturating_sub(18);

        if let Some(proposal) = proposal {
            lines.push(TLine::from(""));
            lines.push(TLine::from(Span::styled("  Proposed change", dim)));
            for row in proposal_detail(proposal) {
                lines.push(TLine::from(vec![
                    Span::styled(format!("{:>16}  ", row.label), dim),
                    Span::raw(clip(&row.value, value_width)),
                ]));
            }
            let hint = if proposal.is_applicable() {
                "                  a to apply · n to decline"
            } else {
                "                  checks failed · review diagnostics above"
            };
            lines.push(TLine::from(Span::styled(hint, dim)));
        }

        if notes.is_empty() {
            return;
        }
        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled("  Learned", dim)));
        for row in note_rows(notes) {
            lines.push(TLine::from(vec![
                Span::styled(format!("{:>16}  ", row.detail), dim),
                Span::raw(clip(&row.label, value_width)),
            ]));
        }
    }
}
