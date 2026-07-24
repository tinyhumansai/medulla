//! Registered worker list and fleet actions.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::super::super::types::App;

impl App {
    /// Draw registered workers with selection and harness identity.
    pub(super) fn draw_workers(&mut self, f: &mut Frame, area: Rect) {
        let workers = self.runtime.workers();
        let selected = self.worker_index.min(workers.len().saturating_sub(1));
        self.worker_index = selected;
        let block = self.panel(format!("List Workers · {}", workers.len()));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let mut lines = Vec::new();
        if workers.is_empty() {
            lines.push(TLine::from(Span::styled(
                "No workers registered. Open Add Worker to connect a remote peer.",
                Style::default().add_modifier(Modifier::DIM),
            )));
        } else {
            let visible = self.visible_count();
            let start = selected
                .saturating_sub(visible / 2)
                .min(workers.len().saturating_sub(visible));
            for (index, worker) in workers.iter().enumerate().skip(start).take(visible) {
                let selected_default = if worker.selected { "●" } else { " " };
                let handle = worker.handle.as_deref().unwrap_or(&worker.address);
                let label = worker
                    .label
                    .as_deref()
                    .map(|value| format!(" · {value}"))
                    .unwrap_or_default();
                let harness = worker
                    .harness
                    .as_deref()
                    .map(|value| format!(" · {}", value.to_uppercase()))
                    .unwrap_or_default();
                let mut style = if worker.selected {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default()
                };
                if index == selected {
                    style = self.theme.selection();
                }
                lines.push(TLine::from(Span::styled(
                    format!(
                        "{selected_default} {} · {handle}{label}{harness}",
                        worker.id
                    ),
                    style,
                )));
            }
        }
        if let Some(identity) = &self.snapshot.tinyplace {
            lines.push(TLine::from(""));
            lines.push(TLine::from(vec![
                Span::styled("this hub · ", Style::default().fg(Color::Cyan)),
                Span::raw(identity.agent_id.clone()),
            ]));
        }
        lines.push(TLine::from(Span::styled(
            "↑↓/jk browse · a add · Enter/s select · e edit label · d/x remove",
            Style::default().add_modifier(Modifier::DIM),
        )));
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }
}
