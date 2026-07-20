//! The two Settings > DEBUG subpages: Trace (the raw trace event stream) and
//! Context (the environment chunks assembled for the model).
//!
//! Both are diagnostic views of the current session rather than settings, which
//! is why they sit under their own heading.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::ui::events::{EventEnvelope, TuiEvent};

use super::super::super::types::App;

impl App {
    /// Draw the Trace subpage: the filtered trace events plus the first event's
    /// JSON.
    pub(super) fn draw_trace(&mut self, f: &mut Frame, area: Rect) {
        let source: Vec<&EventEnvelope> = self
            .snapshot
            .events
            .iter()
            .filter(|e| matches!(e.event, TuiEvent::Trace { .. }))
            .collect();
        let block = self.panel(format!("Trace · {} events", source.len()));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let vis = (inner.height as usize).max(1);
        let start = self.selected.min(source.len().saturating_sub(vis));
        let page: Vec<&EventEnvelope> = source.into_iter().skip(start).take(vis).collect();
        let mut lines: Vec<TLine> = if page.is_empty() {
            vec![TLine::from(Span::styled(
                "No events yet.",
                Style::default().add_modifier(Modifier::DIM),
            ))]
        } else {
            page.iter()
                .map(|e| self.event_line(e, area.width.saturating_sub(6) as usize, false))
                .collect()
        };
        if let Some(first) = page.first() {
            if let Ok(json) = serde_json::to_string(&first.event) {
                lines.push(TLine::from(Span::styled(
                    json,
                    Style::default().add_modifier(Modifier::DIM),
                )));
            }
        }
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }

    /// Draw the Context subpage: the environment chunk list and the selected
    /// chunk's body.
    pub(super) fn draw_context(&mut self, f: &mut Frame, area: Rect) {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
            .split(area);
        let block = self.panel(format!("Environment · {} chunks", self.contexts.len()));
        let inner = block.inner(cols[0]);
        f.render_widget(block, cols[0]);
        self.hit_context = Some(inner);
        let idx = self
            .context_index
            .min(self.contexts.len().saturating_sub(1));
        let vis = (inner.height as usize).max(1);
        let mut lines: Vec<TLine> = Vec::new();
        for (i, item) in self.contexts.iter().take(vis).enumerate() {
            let mut style = Style::default();
            if i == idx {
                style = self.theme.selection();
            }
            lines.push(TLine::from(Span::styled(
                format!("{} · {}b · {}", item.kind, item.bytes, item.ref_),
                style,
            )));
        }
        if self.contexts.is_empty() {
            lines.push(TLine::from(Span::styled(
                "No context chunks yet.",
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        f.render_widget(Paragraph::new(Text::from(lines)), inner);

        let selected = self.contexts.get(idx);
        let title = selected
            .map(|c| c.ref_.clone())
            .unwrap_or_else(|| "Chunk detail".into());
        let content = selected
            .map(|c| c.content.clone())
            .unwrap_or_else(|| "Select a chunk with j/k.".into());
        f.render_widget(
            Paragraph::new(content)
                .wrap(Wrap { trim: false })
                .block(self.panel(title)),
            cols[1],
        );
    }
}
