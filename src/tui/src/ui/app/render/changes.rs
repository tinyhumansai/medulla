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
        self.draw_changes_into(frame, area, " Git changes · b baseline ");
    }

    /// The same two panes, drawn over the Sessions harness pane.
    ///
    /// Same state, same bindings, different real estate: `d` on a harness row
    /// swaps its terminal for this, so the hint has to say how to get the
    /// terminal back. Nothing is re-collected here — the key that opened the
    /// view already pointed [`GitChangesState`](super::super::changes::GitChangesState)
    /// at this session's launch snapshot.
    pub(in crate::ui::app) fn draw_harness_diff(&mut self, frame: &mut Frame, area: Rect) {
        let title = format!(
            " {} · d harness · b baseline ",
            self.changes.baseline_label()
        );
        self.draw_changes_into(frame, area, &title);
    }

    /// Draw the two Changes panes into `area`, titling the rail with `rail_title`.
    fn draw_changes_into(&mut self, frame: &mut Frame, area: Rect, rail_title: &str) {
        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(area);
        self.note_pane(panes[0]);
        self.note_pane(panes[1]);

        let mut rows = vec![ListItem::new(Line::from(Span::styled(
            format!("From {}", self.changes.baseline_label()),
            Style::default().add_modifier(Modifier::BOLD),
        )))];
        if self.changes.picking_baseline {
            let launch = self
                .changes
                .harness_baseline
                .as_deref()
                .map(|id| id.get(..7).unwrap_or(id))
                .unwrap_or("unavailable");
            rows.push(ListItem::new(format!("Session launch  {launch}")));
            rows.extend(
                self.changes.recent_commits.iter().map(|commit| {
                    ListItem::new(format!("{}  {}", commit.short_id(), commit.subject))
                }),
            );
            rows.push(ListItem::new("Manual commit…"));
        } else {
            rows.push(ListItem::new(Line::from(Span::styled(
                format!("{} commit(s) after baseline", self.changes.commits.len()),
                Style::default().fg(Color::DarkGray),
            ))));
            rows.extend(self.changes.commits.iter().map(|commit| {
                ListItem::new(Line::from(Span::styled(
                    format!("  {commit}"),
                    Style::default().fg(Color::DarkGray),
                )))
            }));
        }
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
            .block(self.panel(if self.changes.picking_baseline {
                " Select baseline "
            } else {
                rail_title
            }))
            .highlight_style(self.theme.selection())
            .highlight_symbol("› ");
        let mut state = ListState::default();
        if self.changes.picking_baseline {
            state.select(Some(self.changes.baseline_index + 1));
        } else if !self.changes.files.is_empty() {
            state.select(Some(self.changes.commits.len() + 3 + self.changes.selected));
        }
        frame.render_stateful_widget(list, panes[0], &mut state);

        let (detail, cursor_row) = self.change_detail_lines();
        let title = self
            .changes
            .selected_path()
            .map_or_else(|| " Diff ".into(), |path| path.display().to_string());
        let content_width = panes[1].width.saturating_sub(2);
        let viewport_height = panes[1].height.saturating_sub(2) as usize;
        // Measure every row the way the widget will wrap it, so the scroll
        // bound and the cursor-following offset both follow Ratatui's word
        // boundary behavior as well as hard wrapping of long diff tokens.
        let heights: Vec<usize> = detail
            .iter()
            .map(|line| wrapped_height(line, content_width))
            .collect();
        let rendered_height: usize = heights.iter().sum();
        let detail = Paragraph::new(detail)
            .block(self.panel(format!(" {title} ")))
            .wrap(Wrap { trim: false });
        self.changes.max_scroll = rendered_height.saturating_sub(viewport_height);
        if let Some(row) = cursor_row {
            self.changes.scroll = follow(&heights, row, viewport_height, self.changes.scroll);
        }
        self.changes.scroll = self.changes.scroll.min(self.changes.max_scroll);
        frame.render_widget(
            detail.scroll((self.changes.scroll.min(u16::MAX as usize) as u16, 0)),
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
                    "No changes from the selected baseline to the current worktree.",
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
            .filter(|comment| comment.anchor == CommentAnchor::File && !comment.outdated)
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
                if here && !comment.outdated {
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

/// Number of terminal rows one logical line occupies in the diff pane.
///
/// Asks Ratatui itself, with the wrapping mode the pane renders with, so the
/// count honors word boundaries as well as the hard wrapping of long diff
/// tokens. The measured paragraph deliberately carries no block: `pane_width`
/// is already the content width inside the borders, and the border rows are
/// not part of the scrollable content.
fn wrapped_height(line: &Line<'static>, pane_width: u16) -> usize {
    Paragraph::new(line.clone())
        .wrap(Wrap { trim: false })
        .line_count(pane_width.max(1))
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
