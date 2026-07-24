//! The Repo tab: local branch/status ledger, selected patch, and recent history.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::ui::util::clip;

use super::super::App;

impl App {
    /// Draw every configured repository without letting one invalid root hide
    /// healthy workspaces beside it.
    pub(super) fn draw_repo(&mut self, frame: &mut Frame, area: Rect) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(8), Constraint::Length(8)])
            .split(area);
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(44), Constraint::Percentage(56)])
            .split(rows[0]);

        let mut ledger = Vec::new();
        let mut flat_index = 0;
        if self.repo.reports.is_empty() {
            ledger.push(Line::from(Span::styled(
                if self.repo.loading {
                    "Refreshing local repositories…"
                } else {
                    "No repository data yet · press r to refresh"
                },
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        for report in &self.repo.reports {
            let root = report.root.to_string_lossy();
            if let Some(snapshot) = &report.snapshot {
                let branch = &snapshot.branch;
                let detached = if branch.detached { "detached " } else { "" };
                ledger.push(Line::from(vec![
                    Span::styled(
                        clip(&root, 34),
                        Style::default()
                            .fg(self.theme.primary)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        format!("{detached}{}", branch.name),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(
                        format!("  ↑{} ↓{}", branch.ahead, branch.behind),
                        Style::default().add_modifier(Modifier::DIM),
                    ),
                ]));
                if snapshot.files.is_empty() {
                    ledger.push(Line::from(Span::styled(
                        "  ✓ clean",
                        Style::default().fg(Color::Green),
                    )));
                }
                for change in &snapshot.files {
                    let selected = flat_index == self.repo.file_index;
                    let mut style = Style::default();
                    if selected {
                        style = self.theme.selection();
                    }
                    let rename = change
                        .original_path
                        .as_ref()
                        .map(|from| format!(" ← {}", from.display()))
                        .unwrap_or_default();
                    ledger.push(Line::from(Span::styled(
                        format!(
                            "{} {} {}{}",
                            if selected { "▸" } else { " " },
                            change.marker(),
                            change.path.display(),
                            rename
                        ),
                        style,
                    )));
                    flat_index += 1;
                }
            } else {
                ledger.push(Line::from(Span::styled(
                    clip(&root, 38),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                )));
                ledger.push(Line::from(Span::styled(
                    format!(
                        "  {}",
                        clip(report.error.as_deref().unwrap_or("inspection failed"), 54)
                    ),
                    Style::default().fg(Color::Red),
                )));
            }
        }
        frame.render_widget(
            Paragraph::new(ledger)
                .block(self.panel(if self.repo.loading {
                    " Git ledger · refreshing… "
                } else {
                    " Git ledger · r refresh · ↑↓ select "
                }))
                .wrap(Wrap { trim: false }),
            columns[0],
        );

        let diff_title = self
            .repo
            .diff_key
            .as_ref()
            .map(|(_, path)| format!(" Diff · {} · PgUp/PgDn ", path.display()))
            .unwrap_or_else(|| " Diff ".into());
        let diff_text = self
            .repo
            .diff_error
            .as_deref()
            .unwrap_or_else(|| {
                if self.repo.diff.is_empty() {
                    "Select a tracked change to inspect its patch"
                } else {
                    &self.repo.diff
                }
            })
            .to_owned();
        frame.render_widget(
            Paragraph::new(diff_text)
                .block(self.panel(diff_title))
                .scroll((self.repo.diff_scroll.min(u16::MAX as usize) as u16, 0))
                .wrap(Wrap { trim: false }),
            columns[1],
        );

        let mut commits = Vec::new();
        for report in &self.repo.reports {
            let Some(snapshot) = &report.snapshot else {
                continue;
            };
            commits.push(Line::from(Span::styled(
                clip(&snapshot.root.to_string_lossy(), 52),
                Style::default()
                    .fg(self.theme.primary)
                    .add_modifier(Modifier::BOLD),
            )));
            for commit in snapshot.commits.iter().take(3) {
                commits.push(Line::from(vec![
                    Span::styled(
                        format!("  {} ", commit.short_id),
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw(clip(&commit.subject, 74)),
                    Span::styled(
                        format!(" · {}", clip(&commit.author, 18)),
                        Style::default().add_modifier(Modifier::DIM),
                    ),
                ]));
            }
        }
        frame.render_widget(
            Paragraph::new(commits)
                .block(self.panel(" Recent commits "))
                .wrap(Wrap { trim: false }),
            rows[1],
        );
    }
}
