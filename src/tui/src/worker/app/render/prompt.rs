//! Shared text-entry overlay for daemon controls.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
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
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(42),
                Constraint::Length(5),
                Constraint::Percentage(42),
            ])
            .split(area);
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(18),
                Constraint::Percentage(64),
                Constraint::Percentage(18),
            ])
            .split(rows[1]);
        let target = columns[1];
        f.render_widget(Clear, target);
        let block = self.panel(prompt.title(), true);
        let inner = block.inner(target);
        f.render_widget(block, target);
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    format!("{}▏", prompt.input()),
                    Style::default().fg(Color::White),
                )),
                Line::from(Span::styled(
                    "Enter confirm · Esc cancel",
                    Style::default().fg(Color::DarkGray),
                )),
            ]),
            inner,
        );
    }
}
