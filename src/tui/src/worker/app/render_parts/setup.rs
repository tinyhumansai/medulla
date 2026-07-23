//! The launch setup screen: how this worker runs, then what it runs on.
//!
//! Split from the main render because it is a distinct surface — it replaces the
//! whole frame rather than sharing the chrome, and it is the only screen that
//! exists before the worker is serving anything.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use super::super::types::{SetupStep, WorkerApp, EXECUTION_MODES};
use super::dim;

impl WorkerApp {
    /// The launch step: which harness powers this worker.
    pub(super) fn draw_setup(&mut self, f: &mut Frame, area: Rect) {
        let block = self.panel("Set up this worker", true);
        let inner = block.inner(area);
        f.render_widget(block, area);

        let (question, explain): (&str, [&str; 2]) = match self.setup_step {
            SetupStep::Mode => (
                "How should this worker run the tasks peers send it?",
                [
                    "Headless runs one process per task and narrates itself in",
                    "the log. Interactive runs sessions you can watch and drive.",
                ],
            ),
            SetupStep::Harness => (
                "Which coding agent should power this worker?",
                [
                    "A peer's task frame may name a provider; this is what runs",
                    "when it does not, and what your own sessions open on.",
                ],
            ),
        };
        let mut lines = vec![
            Line::from(Span::styled(
                question,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            dim(explain[0]),
            dim(explain[1]),
            Line::from(""),
        ];

        // `(label, blurb)` for whichever question is showing.
        let options: Vec<(String, String)> = match self.setup_step {
            SetupStep::Mode => EXECUTION_MODES
                .iter()
                .map(|mode| {
                    let label = {
                        let name = mode.as_str();
                        let mut c = name.chars();
                        match c.next() {
                            Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
                            None => name.to_string(),
                        }
                    };
                    (label, mode.blurb().to_string())
                })
                .collect(),
            SetupStep::Harness => self
                .providers
                .iter()
                .map(|p| (p.display_name().to_string(), String::new()))
                .collect(),
        };
        for (i, (label, blurb)) in options.iter().enumerate() {
            let selected = i == self.setup_index;
            let text = format!(
                "  {}  {} {:<12} {}",
                if selected { "▸" } else { " " },
                i + 1,
                label,
                blurb
            );
            let mut style = Style::default().fg(Color::White);
            if selected {
                style = style.add_modifier(Modifier::REVERSED);
            }
            lines.push(Line::from(Span::styled(text.trim_end().to_string(), style)));
        }
        lines.push(Line::from(""));
        // Once the mode is settled it stays on screen, so the second question is
        // answered in the context of the first.
        if self.setup_step == SetupStep::Harness {
            if let Some(mode) = self.mode {
                lines.push(dim(&format!(
                    "running {} · {}",
                    mode.as_str(),
                    mode.blurb()
                )));
            }
        }
        lines.push(dim("↑↓ choose · 1-9 jump · Enter confirm · q quit"));
        f.render_widget(
            Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
            inner,
        );
    }
}
