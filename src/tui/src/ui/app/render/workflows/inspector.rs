//! The node inspector: what the graph cursor is on, in full.
//!
//! A view of its own rather than a strip under the canvas. It holds a node's
//! whole declaration plus, when a run is overlaid, how that run left it — the
//! duration and the diagnostics, which are the reason anyone opens a finished
//! run. That is a screenful, and it used to be shown three rows at a time under
//! a graph it was competing with.
//!
//! `i` opens and closes it; the graph is what it closes back to.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use medulla::ui::workflows::{find_node_in, node_detail, note_rows, proposal_detail, RunOverlay};

use crate::ui::util::clip;

use super::super::super::types::App;

impl App {
    /// Draw the node inspector under the canvas.
    pub(super) fn draw_workflow_inspector(&mut self, f: &mut Frame, area: Rect) {
        // The hint lives in the title so every content row belongs to the node.
        let hint = "i back to the graph";
        let title = match self.selected_graph_node() {
            Some(node) => format!("{} · {} · {hint}", node.name, node.kind),
            None => format!("Node · {hint}"),
        };
        // Focused: the inspector only exists while it is the thing being looked
        // at, so a dim border would say it was somewhere else on screen.
        let block = crate::ui::widgets::panel(&self.theme, title, true);
        let inner = block.inner(area);
        f.render_widget(block, area);
        if inner.height == 0 {
            return;
        }
        let width = inner.width as usize;
        let dim = Style::default().add_modifier(Modifier::DIM);

        let Some(selected) = self.selected_graph_node().cloned() else {
            f.render_widget(
                Paragraph::new(TLine::from(Span::styled(
                    "No node selected. Tab to the canvas and use the arrows.",
                    dim,
                ))),
                inner,
            );
            return;
        };

        let mut lines: Vec<TLine> = Vec::new();
        if let Some(run) = self.selected_workflow_run() {
            let state = RunOverlay::new(run).node(&selected.id);
            let mut text = format!("{} {}", state.state.glyph(), state.state.label());
            if let Some(ms) = state.duration_ms {
                text.push_str(&format!(" · {ms}ms"));
            }
            lines.push(TLine::from(Span::styled(
                clip(&text, width),
                Style::default().fg(super::super::color(state.state.color())),
            )));
            // Diagnostics are why a finished run gets opened at all: an
            // expression that resolved to null is a wiring mistake the run
            // survived and the operator has to find.
            for diagnostic in &state.diagnostics {
                lines.push(TLine::from(Span::styled(
                    clip(&format!("  {diagnostic}"), width),
                    Style::default().fg(ratatui::style::Color::Yellow),
                )));
            }
        }

        if !self.wf.inspector_open {
            // One row: whatever the run said about this node if a run is
            // overlaid, and otherwise what the node is configured to do.
            if lines.is_empty() {
                let mut summary = selected.summary.clone();
                if summary.is_empty() {
                    summary = format!("{} · no configuration", selected.id);
                }
                lines.push(TLine::from(Span::styled(clip(&summary, width), dim)));
            }
            lines.truncate(inner.height as usize);
            f.render_widget(Paragraph::new(Text::from(lines)), inner);
            return;
        }

        // Open: the declaration in full, from the graph rather than the layout —
        // the layout keeps only what a box has room for.
        match self
            .wf
            .graph
            .as_ref()
            .and_then(|graph| find_node_in(graph, &selected.id))
        {
            Some(node) => {
                for row in node_detail(node) {
                    lines.push(TLine::from(vec![
                        Span::styled(format!("{:>16}  ", row.label), dim),
                        Span::raw(clip(&row.value, width.saturating_sub(18))),
                    ]));
                }
            }
            None => lines.push(TLine::from(Span::styled(
                "This node is no longer in the stored graph. Press r.",
                dim,
            ))),
        }
        self.push_learned(&mut lines, width, dim);
        // Scrolled from the top: a long config is read downward, and the
        // interesting fields (id, kind, name) are at the top of it.
        f.render_widget(
            Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
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
        let proposal = self.actionable_proposal();
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
            lines.push(TLine::from(Span::styled(
                "                  a to apply · n to decline",
                dim,
            )));
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
