//! Overlay and composer rendering for [`App`]: the inline prompt overlay, the
//! Chat composer, and the resume-picker modal.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;

use crate::ui::chat::ComposerChrome;

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

    /// Draw the Chat composer.
    ///
    /// The widget itself is [`crate::ui::chat::draw_composer`], shared with the
    /// workflow copilot. What is decided here is only what the orchestrator
    /// means by its three states: focus belongs to the composer whenever the
    /// rail does not hold it, and "busy" is the chat runtime running.
    pub(super) fn draw_composer(&mut self, f: &mut Frame, area: Rect) {
        crate::ui::chat::draw_composer(
            f,
            area,
            &self.draft,
            &self.theme,
            ComposerChrome {
                focused: !self.agents_rail_focused(),
                busy: self.snapshot.running,
                // None: the caption row above already names what Enter submits
                // to, and a placeholder under it would say it twice.
                placeholder: None,
            },
        );
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
