//! The Subconscious tab: Medulla's live, always-on signal field.
//!
//! This is the visible home for the quiet layer below active work. It makes the
//! flow of signals legible without turning the layer into another control room:
//! the graph shows what is moving, while the compact header reports what needs
//! a human's attention.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::ui::stream;

use super::super::types::App;

impl App {
    /// Draw the live signal field and the small set of facts worth escalating.
    pub(super) fn draw_subconscious(&mut self, frame: &mut Frame, area: Rect) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(6), Constraint::Min(0)])
            .split(area);
        self.draw_subconscious_header(frame, rows[0]);
        self.draw_subconscious_graph(frame, rows[1]);
    }

    /// Draw the high-signal summary above the graph, leaving the motion itself
    /// room to breathe rather than competing with a dense dashboard.
    fn draw_subconscious_header(&self, frame: &mut Frame, area: Rect) {
        let decisions = self.decisions().len();
        let active_calls = stream::running_calls(&self.snapshot.events);
        let agents = self
            .snapshot
            .last_result
            .as_ref()
            .map(|result| result.task_ledger.len())
            .unwrap_or(0);
        let decision_line = if decisions == 0 {
            Span::styled(
                "clear · no human decision pending",
                Style::default().fg(Color::Green),
            )
        } else {
            Span::styled(
                format!(
                    "{decisions} decision{} · E review",
                    if decisions == 1 { "" } else { "s" }
                ),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
        };
        let body = Text::from(vec![
            Line::from(Span::styled(
                "SUBCONSCIOUS  ·  LIVE OBSERVATION",
                Style::default()
                    .fg(self.theme.primary)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                Span::styled("● ", Style::default().fg(Color::Green)),
                Span::styled("watching", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw("  signals become work only when they matter"),
            ]),
            Line::from(vec![
                Span::styled(
                    format!("{active_calls} active pulses"),
                    Style::default().fg(Color::LightCyan),
                ),
                Span::raw("  ·  "),
                Span::styled(
                    format!("{agents} agents observed"),
                    Style::default().fg(Color::LightMagenta),
                ),
                Span::raw("  ·  "),
                decision_line,
            ]),
        ]);
        frame.render_widget(
            Paragraph::new(body).block(self.panel("Quietly processing")),
            area,
        );
    }
}
