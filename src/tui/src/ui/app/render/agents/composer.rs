//! The composer under the transcript, and the caption naming what it submits to.
//!
//! The caption is the whole reason this is not just a text box: `Enter` means
//! "instruct the orchestrator" on most rows and "answer this question" on a task
//! that raised one, and an input that does two things must say which.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::ui::agents::AgentRole;

use super::super::super::types::App;
use super::types::Selection;

impl App {
    /// The composer's height: its caption row, the draft's lines, and the border.
    pub(super) fn composer_height(&self) -> u16 {
        (self.draft.text.split('\n').count() as u16).max(1) + 3
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
                .filter(|l| l.role != AgentRole::Orchestrator && selection.node.is_none())
                .map(|l| format!("{} · Enter still instructs the orchestrator", l.label))
                .unwrap_or_else(|| "orchestrator".into()),
        };
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(area);
        f.render_widget(
            Paragraph::new(TLine::from(Span::styled(
                format!(" › {target}"),
                Style::default().add_modifier(Modifier::DIM),
            ))),
            rows[0],
        );
        self.draw_composer(f, rows[1]);
    }
}
