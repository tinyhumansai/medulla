//! Two-pane rendering for changed files, commits, patches, and review comments.
//!
//! Nothing here runs Git or mutates review state; it paints what
//! `app::changes` has already collected and keeps the diff viewport following
//! the review cursor.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use medulla::ui::git_review::CommentAnchor;

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
            let count = self.changes.comments.count_for(&file.path);
            let suffix = if count == 0 {
                String::new()
            } else {
                format!("  💬{count}")
            };
            let origins = file.origin_label();
            ListItem::new(vec![
                Line::from(format!("{}  {}{suffix}", file.status, file.path.display())),
                Line::from(Span::styled(
                    format!("     {origins}"),
                    Style::default().fg(Color::DarkGray),
                )),
            ])
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

        let (detail, cursor_row) = self.change_detail_lines();
        let title = self
            .changes
            .selected_path()
            .map_or_else(|| " Diff ".into(), |path| path.display().to_string());
        let content_width = panes[1].width.saturating_sub(2) as usize;
        let viewport_height = panes[1].height.saturating_sub(2) as usize;
        let heights: Vec<usize> = detail
            .iter()
            .map(|line| wrapped_height(line.width(), content_width))
            .collect();
        let rendered_height: usize = heights.iter().sum();
        self.changes.max_scroll = rendered_height.saturating_sub(viewport_height);
        if let Some(row) = cursor_row {
            self.changes.scroll = follow(&heights, row, viewport_height, self.changes.scroll);
        }
        self.changes.scroll = self.changes.scroll.min(self.changes.max_scroll);
        frame.render_widget(
            Paragraph::new(detail)
                .block(self.panel(format!(" {title} ")))
                .wrap(Wrap { trim: false })
                .scroll((self.changes.scroll.min(u16::MAX as usize) as u16, 0)),
            panes[1],
        );
    }

    /// Build styled diff lines with anchored review comments interleaved.
    ///
    /// Returns the lines plus the row the review cursor occupies, which the
    /// caller uses to keep the cursor inside the viewport.
    fn change_detail_lines(&self) -> (Vec<Line<'static>>, Option<usize>) {
        if let Some(error) = &self.changes.error {
            return (
                vec![Line::from(Span::styled(
                    error.clone(),
                    Style::default().fg(Color::Red),
                ))],
                None,
            );
        }
        if self.changes.files.is_empty() {
            return (
                vec![Line::from(
                    "No changes have been made since this session started.",
                )],
                None,
            );
        }
        let Some(path) = self.changes.selected_path() else {
            return (Vec::new(), None);
        };
        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut cursor_row = None;
        for comment in self
            .changes
            .comments
            .for_path(path)
            .filter(|comment| comment.anchor == CommentAnchor::File)
        {
            lines.push(comment_line("file", &comment.body));
        }
        for (index, text) in self.changes.patch.iter().enumerate() {
            let marker = if index == self.changes.cursor {
                "▸ "
            } else {
                "  "
            };
            let mut style = diff_style(text);
            if index == self.changes.cursor {
                style = self.theme.selection();
                cursor_row = Some(lines.len());
            }
            lines.push(Line::from(vec![
                Span::styled(marker, Style::default().fg(Color::Yellow)),
                Span::styled(text.clone(), style),
            ]));
            let hunk = self
                .changes
                .hunks
                .iter()
                .position(|hunk| hunk.header == index);
            for comment in self.changes.comments.for_path(path) {
                let here = match comment.anchor {
                    CommentAnchor::Line(line) => line == index,
                    CommentAnchor::Hunk(at) => Some(at) == hunk,
                    CommentAnchor::File => false,
                };
                if here {
                    lines.push(comment_line(&comment.anchor.describe(), &comment.body));
                }
            }
        }
        (lines, cursor_row)
    }
}

/// Style one raw patch line by its unified-diff prefix.
fn diff_style(line: &str) -> Style {
    if line.starts_with('+') && !line.starts_with("+++") {
        Style::default().fg(Color::Green)
    } else if line.starts_with('-') && !line.starts_with("---") {
        Style::default().fg(Color::Red)
    } else if line.starts_with("@@") {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    }
}

/// Render one review comment so it cannot be mistaken for repository content.
fn comment_line(anchor: &str, body: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  💬 {anchor}: "),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            body.to_owned(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::ITALIC),
        ),
    ])
}

/// Smallest scroll offset that keeps rendered row `row` inside the viewport.
fn follow(heights: &[usize], row: usize, viewport: usize, current: usize) -> usize {
    let before: usize = heights.iter().take(row).sum();
    let span = heights.get(row).copied().unwrap_or(1);

    // If the row is taller than the viewport, show the top of the row and keep
    // it stable rather than oscillating between top and bottom on every redraw.
    if span > viewport {
        return before;
    }

    // For normal-sized rows, scroll to keep the whole row visible.
    if before < current {
        before
    } else if before + span > current + viewport {
        (before + span).saturating_sub(viewport)
    } else {
        current
    }
}

/// Number of terminal rows a wrapped logical line occupies.
fn wrapped_height(width: usize, pane_width: usize) -> usize {
    width.max(1).div_ceil(pane_width.max(1))
}
