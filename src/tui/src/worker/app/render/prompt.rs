//! Shared text-entry overlay for daemon controls.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

use super::super::types::WorkerApp;

impl WorkerApp {
    /// Draw the active prompt over the reduced main screen.
    pub(super) fn draw_prompt(&self, f: &mut Frame, area: Rect) {
        let Some(prompt) = &self.prompt else {
            return;
        };
        let target = crate::ui::layout::centered_percent(area, 64, 20);
        f.render_widget(Clear, target);
        let block = self.panel(prompt.title.clone(), true);
        let inner = block.inner(target);
        f.render_widget(block, target);
        f.render_widget(
            Paragraph::new(vec![
                crate::ui::widgets::prompt_line(&prompt.draft, &self.theme),
                Line::from(Span::styled(
                    "Enter confirm · Esc cancel",
                    Style::default().add_modifier(Modifier::DIM),
                )),
            ]),
            inner,
        );
    }
}
