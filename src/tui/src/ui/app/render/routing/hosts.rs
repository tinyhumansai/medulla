//! Registered host list and fleet actions.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use medulla::protocol::{BudgetWindow, HarnessBudget, HarnessReadiness};

use super::super::super::types::App;

impl App {
    /// Draw registered hosts as a list above a preview of the selected one.
    ///
    /// The split is what makes roles assignable. Folding every host's capacity,
    /// readiness and budgets inline cost two rows apiece — mostly reading
    /// "details not captured" — and left nowhere for a toggle list that belongs
    /// to *one* host. One row per host up top, everything about the selected one
    /// below, and the preview follows the cursor as it moves.
    pub(super) fn draw_hosts(&mut self, f: &mut Frame, area: Rect) {
        // `Runtime::workers()` keeps the name the serve/hub wire uses; what it
        // returns is the host level of the containment chain, which is what this
        // page shows and what the rest of the UI now calls it.
        let hosts = self.runtime.workers();
        let selected = self.host_index.min(hosts.len().saturating_sub(1));
        self.host_index = selected;

        // Give the preview at most half the page, and never more than the
        // selected host actually has to say. A short roster on a tall terminal
        // should not push its list into a strip.
        let preview_rows = hosts
            .get(selected)
            .map(|host| self.preview_lines(host).len() as u16 + 2)
            .unwrap_or(0)
            .min(area.height / 2);
        let [list_area, preview_area] = Layout::vertical([
            Constraint::Min(3),
            Constraint::Length(if hosts.is_empty() { 0 } else { preview_rows }),
        ])
        .areas(area);

        self.draw_host_list(f, list_area, &hosts, selected);
        if !hosts.is_empty() {
            self.draw_host_preview(f, preview_area, &hosts[selected]);
        }
    }

    /// The roster itself: one row per host, identity only.
    fn draw_host_list(
        &mut self,
        f: &mut Frame,
        area: Rect,
        hosts: &[medulla::runtime::WorkerInfo],
        selected: usize,
    ) {
        let block = self.panel(format!("Hosts · {}", hosts.len()));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let mut lines = Vec::new();
        if hosts.is_empty() {
            lines.push(TLine::from(Span::styled(
                "No hosts registered. Open Add Host to connect a remote machine.",
                dim(),
            )));
        } else {
            // Reserve the footer (and optional hub identity) before choosing the
            // host window so the selected row and action hints stay visible.
            let footer_rows = 1 + usize::from(self.snapshot.link.is_some()) * 2;
            let visible = usize::from(inner.height).saturating_sub(footer_rows).max(1);
            let start = crate::ui::selection::viewport_start(selected, hosts.len(), visible);
            for (index, host) in hosts.iter().enumerate().skip(start).take(visible) {
                let selected_default = if host.selected { "●" } else { " " };
                let handle = host.handle.as_deref().unwrap_or(&host.address);
                let label = host
                    .label
                    .as_deref()
                    .map(|value| format!(" · {value}"))
                    .unwrap_or_default();
                let harness = host
                    .harness
                    .as_deref()
                    .map(|value| format!(" · {}", value.to_uppercase()))
                    .unwrap_or_default();
                let roles = match host.roles.len() {
                    0 => String::new(),
                    1 => " · 1 role".to_string(),
                    count => format!(" · {count} roles"),
                };
                let mut style = if host.selected {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default()
                };
                // While the roles toggle has focus the list still marks its row,
                // but dimly — two lit cursors on one page read as two selections.
                if index == selected {
                    style = if self.host_roles_focus {
                        style.add_modifier(Modifier::BOLD)
                    } else {
                        self.theme.selection()
                    };
                }
                lines.push(TLine::from(Span::styled(
                    format!(
                        "{selected_default} {} · {handle}{label}{harness}{roles}",
                        host.id
                    ),
                    style,
                )));
            }
        }
        if let Some(identity) = &self.snapshot.link {
            lines.push(TLine::from(""));
            lines.push(TLine::from(vec![
                Span::styled("this hub · ", Style::default().fg(Color::Cyan)),
                Span::raw(identity.node_name.clone()),
            ]));
        }
        lines.push(TLine::from(Span::styled(
            "↑↓/jk browse · → roles · r refresh · a add · Enter/s select · e edit · d/x remove",
            dim(),
        )));
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }

    /// Everything known about the selected host, plus its role toggles.
    fn draw_host_preview(
        &mut self,
        f: &mut Frame,
        area: Rect,
        host: &medulla::runtime::WorkerInfo,
    ) {
        let block = self.panel(format!("Host · {}", host.id));
        let inner = block.inner(area);
        f.render_widget(block, area);
        // Draw to the height the pane actually got, which is what lets the role
        // list scroll rather than run off the bottom of a short terminal.
        let lines = self.preview_lines_within(host, Some(inner.height as usize));
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }

    /// The preview at its natural height, used to size the pane.
    fn preview_lines(&self, host: &medulla::runtime::WorkerInfo) -> Vec<TLine<'static>> {
        self.preview_lines_within(host, None)
    }

    /// Build the preview body, windowing the role list when `budget` rows is
    /// less than it needs. Shared with the height calculation so the pane is
    /// sized to what it will actually draw rather than a guess.
    fn preview_lines_within(
        &self,
        host: &medulla::runtime::WorkerInfo,
        budget: Option<usize>,
    ) -> Vec<TLine<'static>> {
        let mut lines = Vec::new();
        lines.push(TLine::from(vec![
            Span::styled("workspace  ", dim()),
            Span::raw(
                host.workspace
                    .clone()
                    .unwrap_or_else(|| "not reported".into()),
            ),
        ]));
        let capacity = match (
            host.ip_address.as_deref(),
            host.cpu_cores,
            host.memory_available_bytes,
            host.memory_total_bytes,
        ) {
            (None, None, None, None) => "details not captured · press r to refresh".into(),
            (ip, cpu, available, total) => format!(
                "IP {} · CPU {} · RAM {} available / {} total",
                ip.unwrap_or("unknown"),
                cpu.map(|cores| format!("{cores} cores"))
                    .unwrap_or_else(|| "unknown".into()),
                available
                    .map(format_bytes)
                    .unwrap_or_else(|| "unknown".into()),
                total.map(format_bytes).unwrap_or_else(|| "unknown".into()),
            ),
        };
        lines.push(TLine::from(vec![
            Span::styled("capacity   ", dim()),
            Span::raw(capacity),
        ]));
        if let Some(note) = readiness_summary(&host.readiness) {
            lines.push(TLine::from(vec![
                Span::styled("harnesses  ", dim()),
                Span::raw(note),
            ]));
        }
        if let Some(note) = budget_summary(&host.budgets) {
            lines.push(TLine::from(vec![
                Span::styled("budgets    ", dim()),
                Span::raw(note),
            ]));
        }
        // Whatever rows the detail above did not use are the role list's to fill.
        let role_budget = budget.map(|rows| rows.saturating_sub(lines.len()));
        lines.extend(self.role_lines(host, role_budget));
        lines
    }

    /// The role toggle list. Roles come from the agent-template catalog, so a
    /// host can only be offered for a role this hub actually knows how to brief.
    ///
    /// `budget` caps the rows; the window follows the cursor so a role can never
    /// be selected but off-screen.
    fn role_lines(
        &self,
        host: &medulla::runtime::WorkerInfo,
        budget: Option<usize>,
    ) -> Vec<TLine<'static>> {
        let templates = self.agent_templates();
        if templates.is_empty() {
            return vec![TLine::from(vec![
                Span::styled("roles      ", dim()),
                Span::styled("no agent templates are declared".to_string(), dim()),
            ])];
        }
        // The summary leads, because "none assigned" is the state most hosts are
        // in and it must not read as a machine excluded from every role. Trailing
        // it under a dozen checkboxes buried exactly the line that says otherwise.
        let summary = if host.roles.is_empty() {
            "none assigned · offered for any role".to_string()
        } else {
            host.roles.join(", ")
        };
        let mut lines = vec![TLine::from(vec![
            Span::styled("roles      ", dim()),
            Span::styled(
                summary,
                if host.roles.is_empty() {
                    dim()
                } else {
                    Style::default().fg(Color::Green)
                },
            ),
        ])];
        // One row is already spent on the summary.
        let visible = budget
            .map(|rows| rows.saturating_sub(1).max(1))
            .unwrap_or(templates.len())
            .min(templates.len());
        let start =
            crate::ui::selection::viewport_start(self.host_role_index, templates.len(), visible);
        for (index, template) in templates.iter().enumerate().skip(start).take(visible) {
            let assigned = host.roles.iter().any(|role| role == &template.id);
            let mark = if assigned { "[x]" } else { "[ ]" };
            let cursor = if self.host_roles_focus && index == self.host_role_index {
                "▸"
            } else {
                " "
            };
            let mut style = if assigned {
                Style::default().fg(Color::Green)
            } else {
                dim()
            };
            if self.host_roles_focus && index == self.host_role_index {
                style = self.theme.selection();
            }
            lines.push(TLine::from(vec![
                Span::styled("           ", dim()),
                Span::styled(format!("{cursor} {mark} {}", template.id), style),
            ]));
        }
        lines
    }
}

/// Format a byte count for a compact host-capacity row.
fn format_bytes(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes >= GIB as u64 {
        format!("{:.1} GiB", bytes as f64 / GIB)
    } else {
        format!("{:.0} MiB", bytes as f64 / MIB)
    }
}

/// Shared subdued style for host detail rows.
fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

/// Strip control characters from probe-supplied text before it reaches a ratatui
/// span. Readiness reasons arrive from a remote host over tiny.place, so a
/// compromised or malicious peer could otherwise smuggle terminal escape/OSC
/// sequences (cursor moves, title rewrites) into the operator's terminal.
fn inline_text(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).collect()
}

/// A compact per-harness readiness line, e.g.
/// `ready claude · not-ready codex (not authenticated)`. `None` when the host
/// advertised no readiness. Display-only; readiness is heuristic and advisory.
/// The reason is untrusted peer text, so it is sanitized before rendering and a
/// reason that sanitizes to empty is dropped.
fn readiness_summary(items: &[HarnessReadiness]) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let parts: Vec<String> = items
        .iter()
        .map(|r| {
            let provider = r.provider.as_str();
            if r.ready {
                format!("ready {provider}")
            } else if let Some(reason) = r
                .reason
                .as_deref()
                .map(inline_text)
                .filter(|s| !s.is_empty())
            {
                format!("not-ready {provider} ({reason})")
            } else {
                format!("not-ready {provider}")
            }
        })
        .collect();
    Some(parts.join(" · "))
}

/// A compact per-harness budget line carrying headroom, window, and cooldown,
/// e.g. `codex 1.5k left (weekly) · claude cooldown 1893456000`. Entries with no
/// usable signal (a pure estimate: no numbers, no window, no cooldown) are
/// omitted; `None` when nothing is worth showing.
fn budget_summary(items: &[HarnessBudget]) -> Option<String> {
    let parts: Vec<String> = items.iter().filter_map(budget_line).collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}

/// One provider's budget segment, or `None` when it carries no usable signal.
fn budget_line(b: &HarnessBudget) -> Option<String> {
    let window = window_label(b.window);
    if b.remaining_tokens.is_none() && b.cooldown_until.is_none() && window.is_none() {
        return None; // a bare estimate — nothing concrete to show.
    }
    let mut seg = b.provider.as_str().to_string();
    if let Some(remaining) = b.remaining_tokens {
        seg.push_str(&format!(" {} left", fmt_tokens(remaining)));
    }
    if let Some(window) = window {
        seg.push_str(&format!(" ({window})"));
    }
    if let Some(until) = b.cooldown_until {
        seg.push_str(&format!(" · cooldown {until}"));
    }
    Some(seg)
}

/// The short label for a metering window, or `None` for `Unknown`.
fn window_label(window: BudgetWindow) -> Option<&'static str> {
    match window {
        BudgetWindow::Daily => Some("daily"),
        BudgetWindow::Weekly => Some("weekly"),
        BudgetWindow::FiveHour => Some("5h"),
        BudgetWindow::Unknown => None,
    }
}

/// Compact token count that scales into thousands/millions (`980` · `1.5k` ·
/// `1.2M`). Negative inputs (never expected) clamp to zero.
fn fmt_tokens(tokens: i64) -> String {
    let tokens = tokens.max(0) as u64;
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        // Keep one fractional digit so `1_500` reads `1.5k`, not a rounded `2k`
        // that would overstate remaining headroom; drop it for whole thousands.
        let thousands = tokens as f64 / 1_000.0;
        if thousands.fract() == 0.0 {
            format!("{}k", thousands as u64)
        } else {
            format!("{thousands:.1}k")
        }
    } else {
        tokens.to_string()
    }
}
