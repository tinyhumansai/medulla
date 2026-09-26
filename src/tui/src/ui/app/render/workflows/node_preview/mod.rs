//! Rich, persistent rendering of the workflow step under the graph cursor.
//!
//! The graph answers where a step sits; this pane answers what it will do.
//! Each common node kind gets a purpose-built presentation, while unknown
//! configuration remains inspectable as redacted, pretty-printed JSON.
//!
//! While a run is in flight the pane leads with [`live`]: the harness output
//! this step is producing right now, which is the only part of the box that
//! changes between frames and therefore the part being watched.

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use medulla::ui::workflows::{find_node_in, RunOverlay};
use medulla::workflows::RunStatus;

use super::super::super::types::App;

mod kinds;
mod live;
mod prompt;
mod run_detail;
mod syntax;
mod transcript;
mod types;

#[cfg(test)]
mod live_tests;
#[cfg(test)]
mod syntax_tests;
#[cfg(test)]
mod tests;

use kinds::kind_lines;
pub(in crate::ui::app::render) use live::live_lines;
use run_detail::{connection_line, run_header, run_lines};
use types::AgentDefaults;

impl App {
    /// Draw the selected node's useful contents below the workflow graph.
    pub(super) fn draw_workflow_node_preview(&mut self, f: &mut Frame, area: Rect) {
        let Some(title) = self.workflow_preview_title(" · wheel/Page scroll · i full") else {
            return;
        };
        let block = crate::ui::widgets::panel(&self.theme, title, false);
        let inner = block.inner(area);
        f.render_widget(block, area);
        self.hit_workflow_preview = Some(inner);
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let lines = self.workflow_preview_lines(inner.width as usize);
        let visible = inner.height as usize;
        let width = inner.width.max(1) as usize;
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

    /// The preview panel's title, or `None` when there is no node to preview.
    ///
    /// `hint` is appended verbatim, so the graph's peek and the full-screen
    /// inspector can name their own way out while saying the same thing about
    /// which step is being looked at.
    pub(in crate::ui::app::render) fn workflow_preview_title(&self, hint: &str) -> Option<String> {
        let selected = self.selected_graph_node()?;
        let run_title = self
            .selected_workflow_run()
            .map(|run| {
                format!(
                    " · run {} {}{}",
                    medulla::ui::workflows::rows::short_run_id(&run.id),
                    medulla::ui::workflows::status_label(run.status),
                    if run.status == RunStatus::Failed {
                        " · f fix via agent"
                    } else {
                        ""
                    }
                )
            })
            .unwrap_or_default();
        Some(format!(
            "{} · {}{run_title}{hint}",
            selected.name, selected.kind
        ))
    }

    /// Everything this pane has to say about the selected step, as lines.
    ///
    /// Separated from the drawing so two callers can share it — the peek under
    /// the graph and the full-screen inspector are the same reading of one node,
    /// differing only in how much room they have for it — and so the canvas can
    /// *measure* it before deciding how many rows to give it. A pane sized to a
    /// fixed fraction spent half the graph's height on a trigger with three
    /// lines to its name.
    pub(in crate::ui::app::render) fn workflow_preview_lines(
        &self,
        width: usize,
    ) -> Vec<Line<'static>> {
        let Some(selected) = self.selected_graph_node().cloned() else {
            return Vec::new();
        };
        let run = self.selected_workflow_run().cloned();
        let Some(config) = self
            .wf
            .graph
            .as_ref()
            .and_then(|graph| find_node_in(graph, &selected.id))
            .map(|node| node.config.clone())
        else {
            return vec![Line::from("Step changed on disk; press r to reload.")];
        };

        let mut lines = Vec::new();
        // First, because it is the answer to "what is happening" — a reader
        // watching a run should not have to scroll past the plan to find it.
        if let Some(live) = self.live_run_view() {
            let live_lines = live_lines(live.frames(&selected.id), live.running);
            if !live_lines.is_empty() {
                lines.extend(live_lines);
                lines.push(Line::from(""));
            }
        }
        if let Some(run) = &run {
            // The run first, then the step. A step's evidence read without
            // knowing what the run was started with is a paragraph out of
            // context — and the inputs are exactly what the operator is
            // comparing two runs of one workflow by.
            lines.extend(run_header(run));
            let state = RunOverlay::new(run).node(&selected.id);
            let duration = state
                .duration_ms
                .map(|ms| format!(" · {ms}ms"))
                .unwrap_or_default();
            lines.push(Line::from(Span::styled(
                format!("{} {}{duration}", state.state.glyph(), state.state.label()),
                Style::default().fg(super::super::color(state.state.color())),
            )));
            for diagnostic in state.diagnostics {
                lines.push(Line::from(Span::styled(
                    format!("  {diagnostic}"),
                    Style::default().fg(Color::Yellow),
                )));
            }
            lines.extend(run_lines(run, &selected.id, selected.kind == "agent"));
            lines.push(Line::from(""));
        }

        lines.push(connection_line(&selected.id, &self.workflow_layout().edges));
        // A step that several identical siblings duplicate says so here, where
        // there is room to name them — the graph only has a badge for it.
        if let Some(note) = self.duplicate_sink_note(&selected.id) {
            lines.push(Line::from(Span::styled(
                note,
                Style::default().fg(Color::Yellow),
            )));
        }
        let agent_defaults = AgentDefaults::new(&self.loaded.config.workflows, &self.wf.defaults);
        lines.extend(kind_lines(&selected.kind, &config, width, &agent_defaults));
        lines
    }

    /// How many rows this pane would like, for a pane of `width` columns.
    ///
    /// Its own content, borders included — never a fraction of the screen. The
    /// caller caps it; this only reports what there is to show.
    pub(in crate::ui::app::render) fn workflow_preview_height(&self, width: usize) -> usize {
        const BORDERS: usize = 2;
        let inner = width.saturating_sub(BORDERS).max(1);
        self.workflow_preview_lines(inner)
            .iter()
            .map(|line| line.width().max(1).div_ceil(inner))
            .sum::<usize>()
            + BORDERS
    }
}
