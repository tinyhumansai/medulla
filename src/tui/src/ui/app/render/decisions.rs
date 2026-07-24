//! Prepared-decision modal rendering.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::ui::decisions::DecisionKind;
use crate::ui::util::clip;

use super::super::types::App;

/// Center a modal within the content area.
fn modal_area(area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(12),
            Constraint::Percentage(76),
            Constraint::Percentage(12),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(10),
            Constraint::Percentage(80),
            Constraint::Percentage(10),
        ])
        .split(vertical[1])[1]
}

impl App {
    /// Draw the current prepared-decision queue over the active tab.
    pub(super) fn draw_decisions(&mut self, f: &mut Frame, area: Rect) {
        let items = self.decisions();
        if items.is_empty() {
            self.decision_open = false;
            return;
        }
        self.decision_index = self.decision_index.min(items.len() - 1);
        let area = modal_area(area);
        f.render_widget(Clear, area);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Yellow))
            .title(Span::styled(
                format!(
                    " Decisions · {}/{} · ↑↓ select · Enter answer · d dismiss · Esc close ",
                    self.decision_index + 1,
                    items.len()
                ),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let selected = &items[self.decision_index];
        let list_height = items.len().min((inner.height as usize / 2).max(2));
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(list_height as u16),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(inner);
        let list = items
            .iter()
            .enumerate()
            .take(list_height)
            .map(|(index, item)| {
                let marker = if index == self.decision_index {
                    "❯"
                } else {
                    " "
                };
                let kind = match item.kind {
                    DecisionKind::WorkerQuestion => "?",
                    DecisionKind::Escalation => "!",
                };
                let style = if index == self.decision_index {
                    self.theme.selection()
                } else {
                    Style::default()
                };
                TLine::from(Span::styled(
                    format!(
                        "{marker} {kind} {} · {}",
                        clip(&item.question, inner.width.saturating_sub(8) as usize),
                        item.lane_context
                    ),
                    style,
                ))
            })
            .collect::<Vec<_>>();
        f.render_widget(Paragraph::new(Text::from(list)), rows[0]);
        f.render_widget(
            Paragraph::new(TLine::from(Span::styled(
                "PREPARED CONTEXT",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ))),
            rows[1],
        );

        let mut detail = vec![
            TLine::from(Span::styled(
                selected.question.clone(),
                Style::default().fg(Color::White),
            )),
            TLine::from(Span::styled(
                selected.lane_context.clone(),
                Style::default().fg(Color::Magenta),
            )),
        ];
        if let Some(excerpt) = &selected.contract_excerpt {
            detail.push(TLine::from(""));
            detail.push(TLine::from(Span::styled(
                format!("boundary · {excerpt}"),
                Style::default().fg(Color::Cyan),
            )));
        }
        if selected.answer_target.is_none() {
            detail.push(TLine::from(""));
            detail.push(TLine::from(Span::styled(
                "informational escalation · Enter or d dismisses",
                Style::default().fg(Color::DarkGray),
            )));
        }
        f.render_widget(Paragraph::new(detail).wrap(Wrap { trim: false }), rows[2]);
    }
}
