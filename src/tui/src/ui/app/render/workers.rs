//! The Workers tab: the registered remote-peer fleet list with selection,
//! harness labels, and the key-hint footer row.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::super::types::App;

impl App {
    /// Draw the Workers tab: the fleet list and its action hints.
    pub(super) fn draw_workers(&mut self, f: &mut Frame, area: Rect) {
        let workers = self.runtime.workers();
        let selected = if workers.is_empty() {
            0
        } else {
            self.worker_index.min(workers.len() - 1)
        };
        self.worker_index = selected;
        let title = format!("Workers · {}", workers.len());
        let block = self.panel(title);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let mut lines: Vec<TLine> = Vec::new();
        if workers.is_empty() {
            lines.push(TLine::from(Span::styled(
                "No workers registered. Press a to add a remote peer (address or @handle).",
                Style::default().add_modifier(Modifier::DIM),
            )));
        } else {
            let vis = self.visible_count();
            let start = selected
                .saturating_sub(vis / 2)
                .min(workers.len().saturating_sub(vis));
            for (i, w) in workers.iter().enumerate().skip(start).take(vis) {
                let marker = if w.selected { "●" } else { " " };
                let handle = w.handle.as_deref().unwrap_or(&w.address);
                let label = w.label.as_deref().unwrap_or("");
                let harness = w
                    .harness
                    .as_deref()
                    .map(|h| format!(" · {}", h.to_uppercase()))
                    .unwrap_or_default();
                let text = format!(
                    "{marker} {} · {}{}{}",
                    w.id,
                    handle,
                    if label.is_empty() {
                        String::new()
                    } else {
                        format!(" · {label}")
                    },
                    harness,
                );
                let mut style = Style::default();
                if w.selected {
                    style = style.fg(Color::Green);
                }
                if i == selected {
                    style = self.theme.selection();
                }
                lines.push(TLine::from(Span::styled(text, style)));
            }
        }
        // This hub's own tiny.place address. A worker only accepts a task from a
        // peer it trusts, so the operator needs this verbatim — rendered in full
        // (never clipped) so it can be copied into the worker's config.
        if let Some(me) = &self.snapshot.tinyplace {
            lines.push(TLine::from(""));
            lines.push(TLine::from(vec![
                Span::styled("this hub · ", Style::default().fg(Color::Cyan)),
                Span::raw(me.agent_id.clone()),
            ]));
            lines.push(TLine::from(Span::styled(
                "set on each worker as TINYPLACE_OPENHUMAN_OWNER (and allowlist it) so it accepts tasks",
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        lines.push(TLine::from(Span::styled(
            "a add · Enter/s select · e edit label · d/x remove",
            Style::default().add_modifier(Modifier::DIM),
        )));
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }
}
