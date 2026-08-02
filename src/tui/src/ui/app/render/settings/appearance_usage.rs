//! Two Settings subpages: Appearance (the theme role editor) and Usage (this
//! session's token spend plus the account totals).

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::ui::stream;
use crate::ui::theme::{color_to_string, THEME_ROLES};
use crate::ui::util::clip;

use super::super::super::appearance::APPEARANCE_ROWS;
use super::super::super::types::App;

impl App {
    /// Draw the Appearance subpage: the theme role list with live-color swatches.
    pub(super) fn draw_appearance(&mut self, f: &mut Frame, area: Rect) {
        let block = self.panel("Appearance");
        let inner = block.inner(area);
        f.render_widget(block, area);
        let sel = self.appearance_index.min(APPEARANCE_ROWS - 1);
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
        // Headers, not rows: the cursor indexes the indicators alone, so these
        // are drawn between them purely to say whose usage each group reports.
        // Without the split, "CPU" above "CPU" invites reading the device's
        // load as Medulla's own.
        let header = Style::default().add_modifier(Modifier::DIM);
        for (offset, (label, value)) in [
            ("Process CPU", self.loaded.config.appearance.cpu),
            ("Process RAM", self.loaded.config.appearance.ram),
            ("Process disk I/O", self.loaded.config.appearance.disk_io),
            ("Device CPU", self.loaded.config.appearance.device_cpu),
            ("Device RAM", self.loaded.config.appearance.device_ram),
            ("Device disk", self.loaded.config.appearance.device_disk),
        ]
        .into_iter()
        .enumerate()
        {
            match offset {
                0 => lines.push(TLine::from(Span::styled(
                    "  Medulla process — status line",
                    header,
                ))),
                3 => {
                    lines.push(TLine::from(""));
                    lines.push(TLine::from(Span::styled(
                        "  This device — Agents sidebar",
                        header,
                    )));
                }
                _ => {}
            }
            let index = THEME_ROLES.len() + offset;
            let style = if index == sel {
                self.theme.selection()
            } else {
                Style::default()
            };
            let marker = if index == sel { "▸ " } else { "  " };
            lines.push(TLine::from(Span::styled(
                format!(
                    "{marker}{label:<20} {}",
                    format!("{value:?}").to_ascii_lowercase()
                ),
                style,
            )));
        }
        lines.push(TLine::from(""));
        lines.push(TLine::from(Span::styled(
            "j/k select · ←/→ or Enter change · applies live",
            Style::default().add_modifier(Modifier::DIM),
        )));
        // The harness-row toggles used to sit here. Point at where they went
        // rather than leaving an operator to find the new page by accident.
        lines.push(TLine::from(Span::styled(
            "harness rows are laid out on the Status line page",
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
        // Scroll to keep the selected row visible on short terminals.
        let height = inner.height as usize;
        let scroll = if sel < height {
            0
        } else {
            sel.saturating_sub(height / 2)
        };
        f.render_widget(
            Paragraph::new(Text::from(lines)).scroll((scroll as u16, 0)),
            inner,
        );
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
            "r refresh · c effective config · 1-9 switch settings pages",
            dim,
        )));
        f.render_widget(
            Paragraph::new(Text::from(lines))
                .wrap(Wrap { trim: false })
                .block(self.panel("Usage")),
            area,
        );
    }
}
