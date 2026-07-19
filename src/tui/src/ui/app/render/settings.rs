//! The Settings tab: the subpage left-nav plus its four subpages — Usage
//! (session + account spend), Appearance (theme role editor), Config (effective
//! configuration), and Help (keyboard & REPL reference).

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::ui::stream;
use crate::ui::theme::{color_to_string, THEME_ROLES};
use crate::ui::util::clip;

use super::super::types::{App, SETTINGS_SUBPAGES, SP_APPEARANCE, SP_CONFIG, SP_USAGE};

impl App {
    /// Draw the Settings tab: subpage nav on the left, the active subpage right.
    pub(super) fn draw_settings(&mut self, f: &mut Frame, area: Rect) {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(16), Constraint::Min(0)])
            .split(area);

        // Left nav: subpage list.
        let block = self.panel("Settings");
        let inner = block.inner(cols[0]);
        f.render_widget(block, cols[0]);
        let mut lines: Vec<TLine> = Vec::new();
        for (i, name) in SETTINGS_SUBPAGES.iter().enumerate() {
            let style = if i == self.settings_index {
                self.theme.selection()
            } else {
                Style::default()
            };
            lines.push(TLine::from(Span::styled(
                format!(" {} {name} ", i + 1),
                style,
            )));
        }
        lines.push(TLine::from(Span::styled(
            "↑↓ nav · 1-4 jump",
            Style::default().add_modifier(Modifier::DIM),
        )));
        f.render_widget(Paragraph::new(Text::from(lines)), inner);

        // Right content: the active subpage.
        match self.settings_index {
            SP_USAGE => self.draw_usage(f, cols[1]),
            SP_APPEARANCE => self.draw_appearance(f, cols[1]),
            SP_CONFIG => self.draw_config(f, cols[1]),
            _ => self.draw_help(f, cols[1]),
        }
    }

    /// Draw the Appearance subpage: the theme role list with live-color swatches.
    pub(super) fn draw_appearance(&mut self, f: &mut Frame, area: Rect) {
        let block = self.panel("Appearance");
        let inner = block.inner(area);
        f.render_widget(block, area);
        let sel = self.appearance_index.min(THEME_ROLES.len() - 1);
        let mut lines: Vec<TLine> = Vec::new();
        for (i, role) in THEME_ROLES.iter().enumerate() {
            let c = self.theme.role(i);
            let text_style = if i == sel {
                self.theme.selection()
            } else {
                Style::default()
            };
            let marker = if i == sel { "▸ " } else { "  " };
            lines.push(TLine::from(vec![
                Span::styled(marker, text_style),
                Span::styled("███ ", Style::default().fg(c)),
                Span::styled(format!("{role:<13} {}", color_to_string(c)), text_style),
            ]));
        }
        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled(
            "j/k select role · ←/→ or Enter cycle color · applies live",
            Style::default().add_modifier(Modifier::DIM),
        )));
        let where_saved = match &self.config_path {
            Some(p) => format!("saved to {}", p.display()),
            None => "changes apply live (no config path set)".into(),
        };
        lines.push(TLine::from(Span::styled(
            where_saved,
            Style::default().add_modifier(Modifier::DIM),
        )));
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }

    /// Draw the Usage subpage: this session's token usage and the account totals.
    pub(super) fn draw_usage(&mut self, f: &mut Frame, area: Rect) {
        let fold = stream::usage_fold(&self.snapshot.events);
        let dim = Style::default().add_modifier(Modifier::DIM);
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let mut lines: Vec<TLine> = Vec::new();
        lines.push(TLine::from(Span::styled("This session", bold)));
        let mut tiers: Vec<(&String, &stream::TierUsage)> = fold.tiers.iter().collect();
        tiers.sort_by(|a, b| a.0.cmp(b.0));
        if tiers.is_empty() && fold.subagent.calls == 0 {
            lines.push(TLine::from(Span::styled("no model calls yet", dim)));
        }
        for (tier, t) in tiers {
            lines.push(TLine::from(format!(
                "{tier:<14} in {:<10} out {:<10} calls {}",
                t.input_tokens, t.output_tokens, t.calls
            )));
        }
        if fold.subagent.calls > 0 {
            lines.push(TLine::from(format!(
                "{:<14} in {:<10} out {:<10} tasks {}",
                "sub-agents",
                fold.subagent.input_tokens,
                fold.subagent.output_tokens,
                fold.subagent.calls
            )));
            for (task, input, output) in fold.tasks.iter().take(12) {
                lines.push(TLine::from(Span::styled(
                    format!("  {} in {input} out {output}", clip(task, 28)),
                    dim,
                )));
            }
        }
        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled("Account", bold)));
        match &self.account_usage {
            None => lines.push(TLine::from(Span::styled(
                "account usage requires backend login (medulla login) · r to refresh",
                dim,
            ))),
            Some(data) => {
                let g = |path: &[&str]| -> Option<serde_json::Value> {
                    let mut cur = data;
                    for key in path {
                        cur = cur.get(key)?;
                    }
                    Some(cur.clone())
                };
                if let Some(plan) = g(&["plan"]).and_then(|v| v.as_str().map(str::to_string)) {
                    lines.push(TLine::from(format!("plan       {plan}")));
                }
                if let Some(spent) = g(&["inferenceTotals", "spent"]).and_then(|v| v.as_f64()) {
                    let calls = g(&["inferenceTotals", "calls"])
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    lines.push(TLine::from(format!(
                        "cycle      ${spent:.4} spent · {calls} calls"
                    )));
                }
                if let Some(remaining) = g(&["remainingUsd"]).and_then(|v| v.as_f64()) {
                    lines.push(TLine::from(format!("remaining  ${remaining:.4}")));
                }
                if let Some(models) = g(&["inferenceByModel"]).and_then(|v| match v {
                    serde_json::Value::Array(rows) => Some(rows),
                    _ => None,
                }) {
                    for row in models.iter().take(8) {
                        let model = row
                            .get("model")
                            .or_else(|| row.get("_id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("?");
                        let spent = row.get("spent").and_then(|v| v.as_f64()).unwrap_or(0.0);
                        lines.push(TLine::from(Span::styled(
                            format!("  {model:<24} ${spent:.4}"),
                            dim,
                        )));
                    }
                }
            }
        }
        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled(
            "r refresh · c effective config · 1-4 switch settings pages",
            dim,
        )));
        f.render_widget(
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: false })
                .block(self.panel("Usage")),
            area,
        );
    }

    /// Draw the Config subpage: the effective configuration and its sources.
    pub(super) fn draw_config(&mut self, f: &mut Frame, area: Rect) {
        let sources = if self.loaded.sources.is_empty() {
            "built-in defaults".to_string()
        } else {
            self.loaded.sources.join(" < ")
        };
        let body = format!("Sources: {sources}\n\n{}", self.loaded.pretty_json());
        let block = self.panel(format!("Configuration · {}", self.loaded.path));
        f.render_widget(
            Paragraph::new(body).wrap(Wrap { trim: false }).block(block),
            area,
        );
    }

    /// Draw the Help subpage: the keyboard and REPL command reference.
    pub(super) fn draw_help(&mut self, f: &mut Frame, area: Rect) {
        let dim = Style::default().add_modifier(Modifier::DIM);
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let lines = vec![
            TLine::from("Tab / Shift-Tab switch views · Chat: type to compose, ↑↓ recall prompt history"),
            TLine::from(Span::styled(
                "In a multi-line draft ↑↓ walk the caret between rows; history recalls from the edge rows",
                dim,
            )),
            TLine::from("Chat pins to the latest reply; the composer is shown only on this view"),
            TLine::from("Enter sends · Shift-Enter inserts a newline (Option-Enter if Shift-Enter sends)"),
            TLine::from("PageUp / PageDown scrolls the Chat and Agents transcripts"),
            TLine::from("Agents: ↑↓ pick an agent · j / k scroll · X cancel task · A answer a question"),
            TLine::from("Workers: a add peer · Enter/s select · e edit label · d/x remove"),
            TLine::from("Context: j / k select chunks · Esc clear input · Ctrl-X abort cycle"),
            TLine::from("Memory: ↑↓ / j k browse directives, facets & hits · /memory <query> to search"),
            TLine::from("Settings: ↑↓ nav subpages · 1-4 jump · Usage/Appearance/Config/Help live here"),
            TLine::from("Appearance: j / k pick a theme role · ←/→ or Enter cycle its color (saved live)"),
            TLine::from("Ctrl-N new session · Ctrl-C quit (nav keys act only when the input is empty)"),
            TLine::from(" "),
            TLine::from(Span::styled("Copy", bold)),
            TLine::from("Ctrl-Y copies the whole chat · /copy last copies just the latest reply"),
            TLine::from(" "),
            TLine::from(Span::styled("Mouse", bold)),
            TLine::from("Click a tab to switch views · in Agents/Context click a row to select · wheel scrolls"),
            TLine::from("Ctrl-O / /mouse release the mouse to the terminal for native drag-select"),
            TLine::from(" "),
            TLine::from(Span::styled("Commands", bold)),
            TLine::from("/new · /fork [name] · /resume · /abort · /clear · /config · /copy [all|last]"),
            TLine::from("/usage · /settings · /theme · /memory [query] · /mouse · /async [on|off] · /help · /quit"),
        ];
        f.render_widget(
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: true })
                .block(self.panel("Keyboard & REPL help")),
            area,
        );
    }
}
