//! The Hooks page: one place to declare lifecycle hooks for every harness
//! Medulla launches, and a live view of what those hooks are reporting.
//!
//! The page is two halves because the question has two halves. The list above
//! answers "what will a harness I start carry?" — including the coverage notes
//! for a hook one harness cannot run, which is otherwise discovered by a hook
//! silently never firing. The feed below answers "and is any of it actually
//! happening?", which for a pty session is the only honest way to know: the
//! reports arrive from the harness itself rather than from Medulla guessing at
//! its screen.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use medulla::harness_hooks::{hook_injection, HookSpec};
use medulla::protocol::HarnessProvider;

use super::super::super::types::App;

/// How many reports the feed shows at most, however tall the pane is.
const FEED_LIMIT: usize = 40;

impl App {
    /// Draw the declared hooks above the reports they are producing.
    pub(super) fn draw_hooks(&mut self, f: &mut Frame, area: Rect) {
        // The feed takes the lower third, and only when there is something in
        // it: an operator who has not started a harness yet should get the whole
        // page for the list they came to edit.
        let reports = self.hook_log.recent(FEED_LIMIT);
        let feed_rows = if reports.is_empty() {
            0
        } else {
            (area.height / 3).max(4)
        };
        let [list_area, feed_area] =
            Layout::vertical([Constraint::Min(3), Constraint::Length(feed_rows)]).areas(area);
        self.draw_hook_list(f, list_area);
        if feed_rows > 0 {
            self.draw_hook_feed(f, feed_area, &reports);
        }
    }

    /// The declared hooks, Medulla's own first.
    fn draw_hook_list(&mut self, f: &mut Frame, area: Rect) {
        let hooks: Vec<HookSpec> = self.hook_rows().to_vec();
        let selected = self.hook_index.min(hooks.len().saturating_sub(1));
        self.hook_index = selected;
        let builtin_on = self.loaded.config.hook_defaults.enabled;

        let block = self.panel(format!("Hooks · {}", hooks.len()));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let dim = Style::default().add_modifier(Modifier::DIM);

        let mut lines: Vec<TLine> = Vec::new();
        if hooks.is_empty() {
            lines.push(TLine::from(
                "No hooks — nothing runs on a harness lifecycle event.",
            ));
            lines.push(TLine::from(""));
            lines.push(TLine::from(Span::styled(
                "Press a to declare one for every harness Medulla launches.",
                dim,
            )));
        } else {
            // Two rows per hook (what it is, then what it runs), plus the
            // footer, which is reserved first so the keys stay visible.
            let visible = usize::from(inner.height)
                .saturating_sub(2)
                .checked_div(2)
                .unwrap_or(0)
                .max(1);
            let start = crate::ui::selection::viewport_start(selected, hooks.len(), visible);
            for (index, hook) in hooks.iter().enumerate().skip(start).take(visible) {
                let mut style = if hook.builtin {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default()
                };
                if index == selected {
                    style = self.theme.selection();
                }
                let marker = if hook.builtin { "◆" } else { "·" };
                lines.push(TLine::from(Span::styled(
                    format!("{marker} {} · {}", hook.event.as_str(), hook.display_name()),
                    style,
                )));

                let mut notes: Vec<String> = Vec::new();
                if hook.builtin {
                    notes.push("Medulla's own".to_string());
                }
                if hook.matcher != "*" {
                    notes.push(format!("matches {}", hook.matcher));
                }
                notes.push(match hook.harnesses.as_slice() {
                    [] => "every harness".to_string(),
                    named => format!(
                        "only {}",
                        named
                            .iter()
                            .map(|provider| provider.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                });
                if let Some(timeout) = hook.timeout() {
                    notes.push(format!("{timeout}s timeout"));
                }
                lines.push(TLine::from(Span::styled(
                    format!("    {}", notes.join(" · ")),
                    dim,
                )));
            }
        }

        // Coverage, once, under the list: which harnesses cannot run what is
        // declared, and what an operator has to do about it. Silence here is
        // exactly the failure this page is for.
        for note in self.hook_coverage_notes() {
            lines.push(TLine::from(Span::styled(
                format!("  ⚠ {note}"),
                Style::default().fg(Color::Yellow),
            )));
        }

        lines.push(TLine::from(Span::styled(
            format!(
                "a add · e edit · d remove · b Medulla's own hooks: {} · Esc back to the menu",
                if builtin_on { "on" } else { "off" }
            ),
            dim,
        )));
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }

    /// What arrived from the harnesses, newest first.
    fn draw_hook_feed(
        &mut self,
        f: &mut Frame,
        area: Rect,
        reports: &[medulla::harness_hooks::HookReport],
    ) {
        let block = self.panel(format!("Reports · {}", self.hook_log.recorded()));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let dim = Style::default().add_modifier(Modifier::DIM);

        let lines: Vec<TLine> = reports
            .iter()
            .take(usize::from(inner.height))
            .map(|report| {
                TLine::from(vec![
                    Span::styled(format!("{:<18}", report.event.as_str()), dim),
                    Span::raw(report.summary.clone()),
                ])
            })
            .collect();
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }

    /// One line per harness that cannot run what is declared, or will not run it
    /// until it is trusted.
    ///
    /// Summarized per harness rather than per hook. The underlying notes are one
    /// per hook — right for a log, wrong here, where eight identical "OpenCode
    /// has no declarative hooks" lines bury the one warning an operator can act
    /// on. Each is clipped to its first sentence for the same reason: the full
    /// reasoning is in the log, and a line that runs off the pane says less than
    /// a short one.
    fn hook_coverage_notes(&self) -> Vec<String> {
        let mut notes: Vec<String> = Vec::new();
        for provider in [
            HarnessProvider::Claude,
            HarnessProvider::Codex,
            HarnessProvider::Opencode,
        ] {
            let injection = hook_injection(provider, &self.loaded.config.hooks);
            if let Some(first) = injection.dropped.first() {
                notes.push(format!(
                    "{} · {} hook{} not installed: {}",
                    provider.as_str(),
                    injection.dropped.len(),
                    if injection.dropped.len() == 1 {
                        ""
                    } else {
                        "s"
                    },
                    first_sentence(&first.reason),
                ));
            }
            for warning in &injection.warnings {
                notes.push(format!(
                    "{} · {}",
                    provider.as_str(),
                    first_sentence(warning)
                ));
            }
        }
        notes
    }
}

/// The first sentence of `text`, or all of it when it has only one.
fn first_sentence(text: &str) -> String {
    match text.find(". ") {
        Some(end) => text[..=end].trim().to_string(),
        None => text.to_string(),
    }
}
