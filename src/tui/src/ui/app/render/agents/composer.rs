//! The composer under the transcript: the caption naming what it submits to,
//! the input itself, and the command peek that opens over it.
//!
//! The caption is the whole reason this is not just a text box: `Enter` means
//! "instruct the orchestrator" on most rows and "answer this question" on a task
//! that raised one, and an input that does two things must say which.
//!
//! The peek is the same idea one step earlier. Typing `/` opens the catalog
//! filtered by what you have typed so far, so the commands are discoverable from
//! the place you would use them rather than from a help page you have to know to
//! open.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

use crate::ui::agents::AgentRole;

use super::super::super::types::App;
use super::types::Selection;

impl App {
    /// The composer's height in a column `width` columns wide: the peek (when
    /// open), the caption row, the draft's rendered rows, and the border.
    ///
    /// `width` is needed because the draft soft-wraps: the same text is two rows
    /// in a narrow column and one in a wide one.
    pub(super) fn composer_height(&self, width: u16) -> u16 {
        const CAPTION: u16 = 1;
        crate::ui::chat::composer_height(&self.draft.text, width) + CAPTION + self.peek_height()
    }

    /// Rows the command peek claims, or zero when it is closed.
    ///
    /// Capped so a long catalog cannot swallow the transcript above it; the
    /// selection scrolls within whatever it gets.
    pub(super) fn peek_height(&self) -> u16 {
        const MAX_ROWS: u16 = 8;
        match self.command_suggestions() {
            Some(specs) => (specs.len().max(1) as u16).min(MAX_ROWS) + 2,
            None => 0,
        }
    }

    /// Draw the composer with a caption naming its target.
    pub(super) fn draw_agent_composer(&mut self, f: &mut Frame, area: Rect, selection: &Selection) {
        let target = match selection
            .task
            .as_ref()
            .filter(|task| task.question_id.is_some())
        {
            Some(task) => format!("answering {}", task.task_id),
            None => selection
                .lane()
                .filter(|l| l.role != AgentRole::Orchestrator)
                .map(|l| format!("{} · Enter still instructs the orchestrator", l.label))
                .unwrap_or_else(|| "orchestrator".into()),
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(self.peek_height()),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(area);
        self.draw_command_peek(f, rows[0]);
        // The caption doubles as the only place the focus keys are written down.
        // Alt+Arrow is documented in the status bar and Help, but a terminal
        // that cannot send it leaves the rail unreachable, so the key that
        // always works is named right where the cursor is.
        let caption = if self.agents_rail_focused() {
            "↑↓ walk agents · K kill session · Enter or type to write".to_string()
        } else {
            format!("› {target}   ·  Esc to pick an agent")
        };
        f.render_widget(
            Paragraph::new(TLine::from(Span::styled(
                format!(" {caption}"),
                Style::default().add_modifier(Modifier::DIM),
            ))),
            rows[1],
        );
        self.draw_composer(f, rows[2]);
    }

    /// Draw the command peek above the composer.
    fn draw_command_peek(&mut self, f: &mut Frame, area: Rect) {
        let Some(specs) = self.command_suggestions() else {
            return;
        };
        if area.height == 0 {
            return;
        }
        let block = crate::ui::widgets::panel(&self.theme, "Commands", false);
        let inner = block.inner(area);
        f.render_widget(Clear, area);
        f.render_widget(block, area);
        if specs.is_empty() {
            f.render_widget(
                Paragraph::new(TLine::from(Span::styled(
                    "  no command matches — Esc to clear",
                    Style::default().add_modifier(Modifier::DIM),
                ))),
                inner,
            );
            return;
        }

        let selected = self.command_index.min(specs.len() - 1);
        let capacity = (inner.height as usize).max(1);
        let start = crate::ui::selection::viewport_start(selected, specs.len(), capacity);
        // The usage column is padded to the widest visible entry so the
        // descriptions line up into a readable second column.
        let gutter = specs
            .iter()
            .skip(start)
            .take(capacity)
            .map(|spec| spec.usage().chars().count())
            .max()
            .unwrap_or(0);
        let lines: Vec<TLine> = specs
            .iter()
            .skip(start)
            .take(capacity)
            .enumerate()
            .map(|(offset, spec)| {
                let active = start + offset == selected;
                let usage = format!("{:<gutter$}", spec.usage());
                let style = if active {
                    self.theme.selection()
                } else {
                    Style::default().fg(self.theme.primary)
                };
                TLine::from(vec![
                    Span::styled(format!("{} {usage}", if active { "▸" } else { " " }), style),
                    Span::styled(
                        format!("  {}", spec.description),
                        if active {
                            style
                        } else {
                            Style::default().add_modifier(Modifier::DIM)
                        },
                    ),
                ])
            })
            .collect();
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }
}
