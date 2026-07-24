//! Rendering for the durable local Tasks tab.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use super::super::App;
use crate::ui::util::clip;

impl App {
    /// Render local tasks in a selectable list with a description detail pane.
    pub(super) fn draw_tasks(&mut self, frame: &mut Frame, area: Rect) {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
            .split(area);
        let mut list = Vec::new();
        if self.tasks.tasks.is_empty() {
            list.push(Line::from(Span::styled(
                "No tasks yet · add tasks in the local repository",
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        for (index, task) in self.tasks.tasks.iter().enumerate() {
            let selected = index == self.selected.min(self.tasks.tasks.len().saturating_sub(1));
            let style = if selected {
                self.theme.selection()
            } else {
                Style::default()
            };
            list.push(Line::from(vec![
                Span::styled(if selected { "▸ " } else { "  " }, style),
                Span::styled(clip(&task.title, 28), style),
                Span::styled(
                    format!("  [{:?}]", task.status),
                    Style::default().add_modifier(Modifier::DIM),
                ),
            ]));
        }
        frame.render_widget(
            Paragraph::new(list).block(Block::default().borders(Borders::ALL).title("Tasks")),
            columns[0],
        );
        let detail = self.tasks.tasks.get(self.selected.min(self.tasks.tasks.len().saturating_sub(1)))
            .map(|task| format!("{}\n\n{}\n\nstatus: {:?}\nsource: {}\ncreated: {}\nupdated: {}\nlast sync: {}", task.title, task.description, task.status, task.source.as_ref().map(|s| format!("{}:{}", s.provider, s.source_id)).unwrap_or_else(|| "local".into()), task.created_at, task.updated_at, task.last_synced_at.as_deref().unwrap_or("never")))
            .unwrap_or_else(|| "Select a task to view its details.\n\nSources are configured in tasks.json under the Medulla home.".into());
        frame.render_widget(
            Paragraph::new(detail)
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::ALL).title("Details")),
            columns[1],
        );
    }
}
