//! The Feedback tab: a header showing the active query and controls, the board
//! list on the left, and the selected item's body plus comments on the right.
//!
//! When the runtime has no board (the local/core runtimes, or a signed-out
//! session) the whole tab collapses to a single hint panel rather than showing
//! an empty list, which would read as "no one has filed anything".

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use medulla::client::{FeedbackItem, FeedbackStatus, FeedbackType};

use crate::ui::util::clip;

use super::super::feedback::sort_label;
use super::super::types::App;

/// The colour a status badge renders in.
fn status_style(status: FeedbackStatus) -> Style {
    match status {
        FeedbackStatus::Completed => Style::default().fg(Color::Green),
        FeedbackStatus::Planned => Style::default().fg(Color::Cyan),
        FeedbackStatus::Open => Style::default().add_modifier(Modifier::DIM),
        FeedbackStatus::Other => Style::default().add_modifier(Modifier::DIM),
    }
}

/// The colour a type badge renders in.
fn kind_style(kind: FeedbackType) -> Style {
    match kind {
        FeedbackType::Bug => Style::default().fg(Color::Red),
        _ => Style::default().fg(Color::Magenta),
    }
}

/// The score cell, tinted by which way this user voted so their own vote is
/// legible at a glance.
fn score_span(item: &FeedbackItem) -> Span<'static> {
    let (marker, style) = match item.my_vote {
        1 => ("▲", Style::default().fg(Color::Green)),
        -1 => ("▼", Style::default().fg(Color::Red)),
        _ => ("·", Style::default().add_modifier(Modifier::DIM)),
    };
    Span::styled(format!("{marker}{:>4} ", item.score), style)
}

impl App {
    /// Draw the Feedback tab.
    pub(super) fn draw_feedback(&mut self, f: &mut Frame, area: Rect) {
        if !self.feedback.supported {
            self.draw_feedback_unavailable(f, area);
            return;
        }

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(4), Constraint::Min(0)])
            .split(area);

        self.draw_feedback_header(f, rows[0]);

        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(46), Constraint::Percentage(54)])
            .split(rows[1]);

        self.draw_feedback_list(f, cols[0]);
        self.draw_feedback_detail(f, cols[1]);
    }

    /// The sign-in hint shown when this runtime has no board.
    fn draw_feedback_unavailable(&mut self, f: &mut Frame, area: Rect) {
        let dim = Style::default().add_modifier(Modifier::DIM);
        let lines = vec![
            TLine::from(Span::styled(
                "The feedback board needs a signed-in backend connection.",
                Style::default().fg(Color::Yellow),
            )),
            TLine::from(""),
            TLine::from(Span::styled(
                "Sign in with `medulla login` (or set MEDULLA_TOKEN), then reopen this tab",
                dim,
            )),
            TLine::from(Span::styled(
                "to browse, vote on, and comment on what everyone is asking for.",
                dim,
            )),
        ];
        f.render_widget(
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: true })
                .block(self.panel("Feedback")),
            area,
        );
    }

    /// The query summary and the tab's key hints.
    fn draw_feedback_header(&mut self, f: &mut Frame, area: Rect) {
        let q = &self.feedback.query;
        let filter = match q.kind {
            None => "all",
            Some(FeedbackType::Bug) => "bugs",
            _ => "features",
        };
        let count = if self.feedback.loading {
            "loading…".to_string()
        } else {
            format!("{} item(s)", self.feedback.total)
        };
        let summary = TLine::from(vec![
            Span::styled(
                format!("sort {}", sort_label(q.sort)),
                Style::default().fg(self.theme.primary),
            ),
            Span::raw(" · "),
            Span::styled(
                format!("filter {filter}"),
                Style::default().fg(self.theme.primary),
            ),
            Span::raw(" · "),
            Span::raw(count),
        ]);
        let hints = TLine::from(Span::styled(
            "↑/↓ select · u upvote · d downvote · c comment · n feature · b bug · s sort · f filter · r refresh",
            Style::default().add_modifier(Modifier::DIM),
        ));
        f.render_widget(
            Paragraph::new(Text::from(vec![summary, hints]))
                .wrap(Wrap { trim: true })
                .block(self.panel("Feedback board")),
            area,
        );
    }

    /// The scrollable board list.
    fn draw_feedback_list(&mut self, f: &mut Frame, area: Rect) {
        let block = self.panel("Board");
        let inner = block.inner(area);
        f.render_widget(block, area);

        let items = &self.feedback.items;
        if items.is_empty() {
            let msg = if self.feedback.loading {
                "Loading the board…"
            } else {
                "Nothing here yet. Press n to request a feature or b to report a bug."
            };
            f.render_widget(
                Paragraph::new(Text::from(TLine::from(Span::styled(
                    msg,
                    Style::default().add_modifier(Modifier::DIM),
                ))))
                .wrap(Wrap { trim: true }),
                inner,
            );
            return;
        }

        let idx = self.feedback.index.min(items.len() - 1);
        // Two rows per item (title, then the badge line), so the window is half
        // the available height.
        let vis = ((inner.height as usize) / 2).max(1);
        let start = idx
            .saturating_sub(vis / 2)
            .min(items.len().saturating_sub(vis));

        let mut lines: Vec<TLine> = Vec::new();
        for (i, item) in items.iter().enumerate().skip(start).take(vis) {
            let title_style = if i == idx {
                self.theme.selection()
            } else {
                Style::default()
            };
            lines.push(TLine::from(vec![
                score_span(item),
                Span::styled(clip(&item.title, 34), title_style),
            ]));
            lines.push(TLine::from(vec![
                Span::raw("      "),
                Span::styled(item.kind.label(), kind_style(item.kind)),
                Span::raw(" · "),
                Span::styled(item.status.label(), status_style(item.status)),
                Span::styled(
                    format!(" · {} comment(s)", item.comment_count),
                    Style::default().add_modifier(Modifier::DIM),
                ),
            ]));
        }
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }

    /// The selected item's full body, metadata, and comments.
    fn draw_feedback_detail(&mut self, f: &mut Frame, area: Rect) {
        let dim = Style::default().add_modifier(Modifier::DIM);
        let Some(item) = self.feedback.items.get(self.feedback.index) else {
            f.render_widget(
                Paragraph::new(Text::from(TLine::from(Span::styled(
                    "Select an item with ↑/↓.",
                    dim,
                ))))
                .block(self.panel("Detail")),
                area,
            );
            return;
        };

        let mut body: Vec<TLine> = Vec::new();
        body.push(TLine::from(Span::styled(
            item.title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        body.push(TLine::from(vec![
            Span::styled(item.kind.label(), kind_style(item.kind)),
            Span::raw(" · "),
            Span::styled(item.status.label(), status_style(item.status)),
            Span::styled(
                format!(
                    " · ▲{} ▼{} · by {}",
                    item.upvote_count,
                    item.downvote_count,
                    item.created_by_name.as_deref().unwrap_or("someone")
                ),
                dim,
            ),
        ]));
        if let Some(url) = item.github.as_ref().and_then(|g| g.issue_url.as_deref()) {
            body.push(TLine::from(Span::styled(
                format!("tracked as {url}"),
                Style::default().fg(self.theme.primary),
            )));
        }
        body.push(TLine::from(""));
        for line in item.body.lines() {
            body.push(TLine::from(line.to_string()));
        }

        body.push(TLine::from(""));
        // Comments are fetched per selection, so they may lag the highlighted
        // row by one round-trip.
        let loaded = self.feedback.detail_id.as_deref() == Some(item.id.as_str());
        if !loaded {
            body.push(TLine::from(Span::styled("Loading comments…", dim)));
        } else if self.feedback.comments.is_empty() {
            body.push(TLine::from(Span::styled(
                "No comments yet — press c to add one.",
                dim,
            )));
        } else {
            body.push(TLine::from(Span::styled(
                format!("── {} comment(s) ──", self.feedback.comments.len()),
                dim,
            )));
            for c in &self.feedback.comments {
                body.push(TLine::from(""));
                body.push(TLine::from(Span::styled(
                    c.user_name.clone().unwrap_or_else(|| "someone".into()),
                    Style::default().fg(self.theme.primary),
                )));
                for line in c.body.lines() {
                    body.push(TLine::from(line.to_string()));
                }
            }
        }

        f.render_widget(
            Paragraph::new(Text::from(body))
                .wrap(Wrap { trim: false })
                .scroll((self.feedback.detail_scroll as u16, 0))
                .block(self.panel("Detail")),
            area,
        );
    }
}
