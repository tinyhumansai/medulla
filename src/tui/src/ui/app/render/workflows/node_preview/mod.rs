//! Rich, persistent rendering of the workflow step under the graph cursor.
//!
//! The graph answers where a step sits; this pane answers what it will do.
//! Each common node kind gets a purpose-built presentation, while unknown
//! configuration remains inspectable as redacted, pretty-printed JSON.

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use medulla::ui::workflows::{find_node_in, RunOverlay};
use medulla::workflows::RunStatus;

use super::super::super::types::App;

mod kinds;
mod prompt;
mod run_detail;
mod syntax;
mod types;

#[cfg(test)]
mod syntax_tests;
#[cfg(test)]
mod tests;

use kinds::kind_lines;
use run_detail::{connection_line, run_lines};
use types::AgentDefaults;

impl App {
    /// Draw the selected node's useful contents below the workflow graph.
    pub(super) fn draw_workflow_node_preview(&mut self, f: &mut Frame, area: Rect) {
        let Some(selected) = self.selected_graph_node().cloned() else {
            return;
        };
        let run = self.selected_workflow_run().cloned();
        let run_title = run
            .as_ref()
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
        let block = crate::ui::widgets::panel(
            &self.theme,
            format!(
                "{} · {}{run_title} · wheel/Page scroll · i full",
                selected.name, selected.kind
            ),
            false,
        );
        let inner = block.inner(area);
        f.render_widget(block, area);
        self.hit_workflow_preview = Some(inner);
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let Some(config) = self
            .wf
            .graph
            .as_ref()
            .and_then(|graph| find_node_in(graph, &selected.id))
            .map(|node| node.config.clone())
        else {
            f.render_widget(
                Paragraph::new("Step changed on disk; press r to reload."),
                inner,
            );
            return;
        };

        let mut lines = Vec::new();
        if let Some(run) = &run {
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
        let agent_defaults = AgentDefaults::new(&self.loaded.config.workflows, &self.wf.defaults);
        lines.extend(kind_lines(
            &selected.kind,
            &config,
            inner.width as usize,
            &agent_defaults,
        ));
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
}
