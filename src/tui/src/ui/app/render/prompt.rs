//! Overlay and composer rendering for [`App`]: the inline prompt overlay, the
//! Chat composer, and the resume-picker modal.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;

use crate::ui::composer::caret_row_col;

use super::super::types::App;

impl App {
    /// Draw the inline prompt overlay (Workers add/edit, Agents answer).
    pub(super) fn draw_prompt(&mut self, f: &mut Frame, area: Rect) {
        let Some(prompt) = &self.prompt else { return };
        let block = crate::ui::widgets::panel(&self.theme, prompt.title.clone(), true);
        let inner = block.inner(area);
        f.render_widget(block, area);
        f.render_widget(
            Paragraph::new(crate::ui::widgets::prompt_line(&prompt.draft, &self.theme)),
            inner,
        );
    }

    /// Draw the Chat composer with its caret-highlighted draft lines.
    ///
    /// The caret is drawn solid only while the composer holds the keyboard. When
    /// focus has stepped out to the rail a reversed block would still read as
    /// "your typing goes here", which is exactly the thing that is no longer
    /// true — two visible carets and no way to tell which one is live.
    pub(super) fn draw_composer(&mut self, f: &mut Frame, area: Rect) {
        let focused = !self.agents_rail_focused();
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(if self.snapshot.running {
                Color::Yellow
            } else if focused {
                self.theme.primary
            } else {
                self.theme.dim_border
            }));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let caret = caret_row_col(&self.draft.text, self.draft.cursor);
        let mut lines: Vec<TLine> = Vec::new();
        for (index, row) in self.draft.text.split('\n').enumerate() {
            let prefix = if index == 0 { "❯ " } else { "  " };
            let mut spans = vec![Span::styled(
                prefix,
                Style::default().fg(self.theme.primary),
            )];
            if index == caret.row {
                let chars: Vec<char> = row.chars().collect();
                let before: String = chars.iter().take(caret.col).collect();
                let at: String = chars
                    .get(caret.col)
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| " ".into());
                let after: String = chars.iter().skip(caret.col + 1).collect();
                spans.push(Span::raw(before));
                spans.push(Span::styled(
                    at,
                    if focused {
                        Style::default().add_modifier(Modifier::REVERSED)
                    } else {
                        Style::default().add_modifier(Modifier::DIM)
                    },
                ));
                spans.push(Span::raw(after));
            } else {
                spans.push(Span::raw(row.to_string()));
            }
            lines.push(TLine::from(spans));
        }
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }

    /// Draw the resume-picker modal listing resumable chats.
    pub(super) fn draw_resume(&mut self, f: &mut Frame, area: Rect) {
        let Some(picker) = &self.resume_picker else {
            return;
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.theme.accent))
            .title(Span::styled(
                format!(
                    "Resume a chat — ↑/↓ select · Enter load · Esc cancel ({})",
                    picker.chats.len()
                ),
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let cap = (inner.height as usize).max(1);
        let start = crate::ui::selection::viewport_start(picker.index, picker.chats.len(), cap);
        let mut lines = Vec::new();
        for (i, chat) in picker.chats.iter().enumerate().skip(start).take(cap) {
            let marker = if i == picker.index { "❯ " } else { "  " };
            let mut style = Style::default();
            if i == picker.index {
                style = self.theme.selection();
            }
            let text = format!(
                "{marker}{} · {}t · {} thread{} · {}",
                chat.name,
                chat.turns,
                chat.thread_count,
                if chat.thread_count == 1 { "" } else { "s" },
                chat.updated_at,
            );
            lines.push(TLine::from(Span::styled(text, style)));
        }
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }
}
