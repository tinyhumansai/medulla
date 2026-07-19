//! The Chat tab: the threads sidebar plus the wrapped, scroll-anchored
//! transcript with its thinking/scroll status row.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::ui::stream;
use crate::ui::util::SPINNER;

use super::super::types::App;
use super::{chat_lines, styled_to_tline};

impl App {
    /// Draw the Chat tab: threads sidebar and the transcript pane.
    pub(super) fn draw_chat(&mut self, f: &mut Frame, area: Rect) {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(26), Constraint::Min(0)])
            .split(area);

        // Threads sidebar.
        let block = self.panel(format!("Threads · {}", self.snapshot.threads.len()));
        let inner = block.inner(cols[0]);
        f.render_widget(block, cols[0]);
        let cap = (inner.height as usize).saturating_sub(1).max(1);
        let active_idx = self.active_thread_idx();
        let window_start = active_idx
            .saturating_sub(cap / 2)
            .min(self.snapshot.threads.len().saturating_sub(cap));
        self.hit_threads = Some((inner, window_start));
        let depth = stream::thread_depths(&self.snapshot.threads);
        let mut lines: Vec<TLine> = Vec::new();
        for t in self.snapshot.threads.iter().skip(window_start).take(cap) {
            let d = *depth.get(&t.id).unwrap_or(&0);
            let indent = if d == 0 {
                String::new()
            } else {
                format!("{}⑃ ", "  ".repeat(d - 1))
            };
            let marker = if t.running { "▶" } else { "●" };
            let mut badges = Vec::new();
            if t.running_tasks > 0 {
                badges.push(format!("{} run", t.running_tasks));
            }
            if t.attention > 0 {
                badges.push(format!("{}⚠", t.attention));
            }
            let badge = if badges.is_empty() {
                String::new()
            } else {
                format!(" · {}", badges.join(" "))
            };
            let text = format!("{indent}{marker} {} · {}t{badge}", t.name, t.turns);
            let mut style = Style::default();
            if t.running {
                style = style.fg(Color::Yellow);
            }
            if t.id == self.snapshot.active_thread_id {
                style = self.theme.selection();
            }
            lines.push(TLine::from(Span::styled(text, style)));
        }
        lines.push(TLine::from(Span::styled(
            "^F fork · ^↑↓ switch",
            Style::default().add_modifier(Modifier::DIM),
        )));
        f.render_widget(Paragraph::new(Text::from(lines)), inner);

        // Transcript.
        let name = self
            .snapshot
            .threads
            .get(active_idx)
            .map(|t| t.name.clone())
            .unwrap_or_else(|| "main".into());
        let title = format!(
            "{name} · {} turns",
            self.snapshot.messages.len().div_ceil(2)
        );
        let block = self.panel(title);
        let inner = block.inner(cols[1]);
        f.render_widget(block, cols[1]);
        let width = inner.width as usize;
        let capacity = (inner.height as usize).saturating_sub(1).max(4);
        let lines = chat_lines(&self.snapshot.chat_events, width.saturating_sub(2));
        let max_scroll = lines.len().saturating_sub(capacity);
        let eff = self.chat_scroll.min(max_scroll);
        self.chat_scroll = eff;
        let end = lines.len() - eff;
        let view = &lines[end.saturating_sub(capacity)..end];
        let mut out: Vec<TLine> = if view.is_empty() {
            vec![TLine::from(Span::styled(
                "No messages yet — type below to start.",
                Style::default().add_modifier(Modifier::DIM),
            ))]
        } else {
            view.iter().map(styled_to_tline).collect()
        };
        // Status row.
        if eff > 0 {
            out.push(TLine::from(Span::styled(
                format!("↑ {eff} line(s) below · scroll down / PageDown to catch up"),
                Style::default().add_modifier(Modifier::DIM),
            )));
        } else if self.snapshot.running {
            let rc = stream::running_calls(&self.snapshot.events);
            let msg = if rc > 0 {
                format!(
                    "thinking · {rc} model call{} in flight",
                    if rc == 1 { "" } else { "s" }
                )
            } else {
                "working…".into()
            };
            out.push(TLine::from(Span::styled(
                format!("{} {msg}", SPINNER[self.frame % SPINNER.len()]),
                Style::default().fg(Color::Yellow),
            )));
        }
        f.render_widget(Paragraph::new(Text::from(out)), inner);
    }
}
