//! Two-pane rendering for changed files, commits, patches, and review comments.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use super::super::types::App;

impl App {
    /// Draw the session-start summary/file rail and selected unified patch.
    pub(super) fn draw_changes(&mut self, frame: &mut Frame, area: Rect) {
        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(area);
        self.note_pane(panes[0]);
        self.note_pane(panes[1]);

        let mut rows = vec![ListItem::new(Line::from(Span::styled(
            format!("{} commit(s)", self.changes.commits.len()),
            Style::default().add_modifier(Modifier::BOLD),
        )))];
        rows.extend(self.changes.commits.iter().map(|commit| {
            ListItem::new(Line::from(Span::styled(
                format!("  {commit}"),
                Style::default().fg(Color::DarkGray),
            )))
        }));
        rows.push(ListItem::new(""));
        rows.extend(self.changes.files.iter().map(|file| {
            let count = self.changes.comments.get(&file.path).map_or(0, Vec::len);
            let suffix = if count == 0 {
                String::new()
            } else {
                format!("  💬{count}")
            };
            ListItem::new(format!("{}  {}{suffix}", file.status, file.path))
        }));
        let list = List::new(rows)
            .block(self.panel(" Changes since session start "))
            .highlight_style(self.theme.selection())
            .highlight_symbol("› ");
        let mut state = ListState::default();
        if !self.changes.files.is_empty() {
            state.select(Some(self.changes.commits.len() + 2 + self.changes.selected));
        }
        frame.render_stateful_widget(list, panes[0], &mut state);

        let detail = self.change_detail_lines();
        let title = self.changes.selected_path().unwrap_or(" Diff ").to_owned();
        frame.render_widget(
            Paragraph::new(detail)
                .block(self.panel(format!(" {title} ")))
                .wrap(Wrap { trim: false })
                .scroll((self.changes.scroll.min(u16::MAX as usize) as u16, 0)),
            panes[1],
        );
    }

    /// Build styled diff lines followed by any session-local review comments.
    fn change_detail_lines(&self) -> Vec<Line<'static>> {
        if let Some(error) = &self.changes.error {
            return vec![Line::from(Span::styled(
                error.clone(),
                Style::default().fg(Color::Red),
            ))];
        }
        if self.changes.files.is_empty() {
            return vec![Line::from(
                "No changes have been made since this session started.",
            )];
        }
        let mut lines: Vec<Line<'static>> = self
            .changes
            .patch
            .iter()
            .map(|line| {
                let style = if line.starts_with('+') && !line.starts_with("+++") {
                    Style::default().fg(Color::Green)
                } else if line.starts_with('-') && !line.starts_with("---") {
                    Style::default().fg(Color::Red)
                } else if line.starts_with("@@") {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default()
                };
                Line::from(Span::styled(line.clone(), style))
            })
            .collect();
        if let Some(comments) = self
            .changes
            .selected_path()
            .and_then(|path| self.changes.comments.get(path))
        {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Review comments",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.extend(
                comments
                    .iter()
                    .map(|comment| Line::from(format!("• {comment}"))),
            );
        }
        lines
    }
}
