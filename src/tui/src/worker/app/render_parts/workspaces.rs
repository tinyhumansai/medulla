//! Workspace allowlist view.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use super::super::types::WorkerApp;
use super::dim;

impl WorkerApp {
    /// Draw workspace roots this worker may disclose to its master.
    pub(super) fn draw_workspaces(&mut self, f: &mut Frame, area: Rect) {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(area);
        let selected = self
            .workspace_index
            .min(self.workspaces.len().saturating_sub(1));
        self.workspace_index = selected;

        let block = self.panel(
            format!("Allowed workspaces · {}", self.workspaces.len()),
            true,
        );
        let inner = block.inner(columns[0]);
        f.render_widget(block, columns[0]);
        let mut lines = Vec::new();
        for (index, workspace) in self.workspaces.iter().enumerate() {
            let primary = workspace == &self.primary_workspace;
            let mut style = Style::default().fg(if primary { Color::Green } else { Color::White });
            if index == selected {
                style = style.add_modifier(Modifier::REVERSED);
            }
            lines.push(Line::from(Span::styled(
                format!("{} {workspace}", if primary { "●" } else { "○" }),
                style,
            )));
        }
        f.render_widget(
            Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
            inner,
        );

        let block = self.panel("Permission boundary", false);
        let inner = block.inner(columns[1]);
        f.render_widget(block, columns[1]);
        let details = vec![
            Line::from(Span::styled(
                "Only these roots are advertised",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
            dim("The master receives them in this worker's capabilities."),
            dim("The active root is where inbound tasks actually execute."),
            Line::from(""),
            Line::from(vec![
                Span::styled("active  ", Style::default().fg(Color::DarkGray)),
                Span::raw(self.primary_workspace.clone()),
            ]),
            Line::from(vec![
                Span::styled("saved   ", Style::default().fg(Color::DarkGray)),
                Span::raw(self.config_path.display().to_string()),
            ]),
            Line::from(""),
            dim("a allow directory · d stop advertising selected root"),
        ];
        f.render_widget(
            Paragraph::new(Text::from(details)).wrap(Wrap { trim: false }),
            inner,
        );
    }
}
