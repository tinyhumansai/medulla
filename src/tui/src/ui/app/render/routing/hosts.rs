//! Registered host list and fleet actions.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line as TLine, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use medulla::tinyplace::{BudgetWindow, HarnessBudget, HarnessReadiness};

use super::super::super::types::App;

impl App {
    /// Draw registered hosts with selection and harness identity.
    pub(super) fn draw_hosts(&mut self, f: &mut Frame, area: Rect) {
        // `Runtime::workers()` keeps the name the serve/hub wire uses; what it
        // returns is the host level of the containment chain, which is what this
        // page shows and what the rest of the UI now calls it.
        let hosts = self.runtime.workers();
        let selected = self.host_index.min(hosts.len().saturating_sub(1));
        self.host_index = selected;
        let block = self.panel(format!("Hosts · {}", hosts.len()));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let mut lines = Vec::new();
        if hosts.is_empty() {
            lines.push(TLine::from(Span::styled(
                "No hosts registered. Open Add Host to connect a remote machine.",
                Style::default().add_modifier(Modifier::DIM),
            )));
        } else {
            // Each host consumes a summary row plus a capacity-details row.
            // Reserve the footer (and optional hub identity) before choosing the
            // host window so the selected row and action hints stay visible.
            let footer_rows = 1 + usize::from(self.snapshot.tinyplace.is_some()) * 2;
            let visible = usize::from(inner.height)
                .saturating_sub(footer_rows)
                .checked_div(2)
                .unwrap_or(0)
                .max(1);
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
                let mut style = if host.selected {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default()
                };
                if index == selected {
                    style = self.theme.selection();
                }
                lines.push(TLine::from(Span::styled(
                    format!("{selected_default} {} · {handle}{label}{harness}", host.id),
                    style,
                )));
                let details = match (
                    host.ip_address.as_deref(),
                    host.cpu_cores,
                    host.memory_available_bytes,
                    host.memory_total_bytes,
                ) {
                    (None, None, None, None) => {
                        "  details not captured · press r to refresh".into()
                    }
                    (ip, cpu, available, total) => format!(
                        "  IP {} · CPU {} · RAM {} available / {} total",
                        ip.unwrap_or("unknown"),
                        cpu.map(|cores| format!("{cores} cores"))
                            .unwrap_or_else(|| "unknown".into()),
                        available
                            .map(format_bytes)
                            .unwrap_or_else(|| "unknown".into()),
                        total.map(format_bytes).unwrap_or_else(|| "unknown".into()),
                    ),
                };
                // Fold the probe's readiness and budget lines onto the same
                // capacity row so the roster stays two lines per host (its
                // pagination unit). Absent when the host advertised neither.
                let mut details = details;
                if let Some(note) = readiness_summary(&host.readiness) {
                    details.push_str(&format!(" · {note}"));
                }
                if let Some(note) = budget_summary(&host.budgets) {
                    details.push_str(&format!(" · {note}"));
                }
                lines.push(TLine::from(Span::styled(details, dim())));
            }
        }
        if let Some(identity) = &self.snapshot.tinyplace {
            lines.push(TLine::from(""));
            lines.push(TLine::from(vec![
                Span::styled("this hub · ", Style::default().fg(Color::Cyan)),
                Span::raw(identity.agent_id.clone()),
            ]));
        }
        lines.push(TLine::from(Span::styled(
            "↑↓/jk browse · r refresh details · a add · Enter/s select · e edit · d/x remove",
            Style::default().add_modifier(Modifier::DIM),
        )));
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
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
