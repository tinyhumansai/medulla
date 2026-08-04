//! Formatting shared by the host tree and its preview: byte counts, token
//! headroom, the subdued style, and the two probe-reported summaries.

use ratatui::style::{Modifier, Style};

use medulla::protocol::{BudgetWindow, HarnessBudget, HarnessReadiness};

/// Format a byte count for a compact host-capacity row.
pub(super) fn format_bytes(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes >= GIB as u64 {
        format!("{:.1} GiB", bytes as f64 / GIB)
    } else {
        format!("{:.0} MiB", bytes as f64 / MIB)
    }
}

/// Shared subdued style for host detail rows.
pub(super) fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

/// Strip control characters from text before it reaches a ratatui span.
///
/// Readiness reasons, labels, ids and workspace paths arrive from a remote host
/// over tiny.place, so a compromised or malicious peer could otherwise smuggle
/// terminal escape/OSC sequences (cursor moves, title rewrites) into the
/// operator's terminal.
pub(super) fn inline_text(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).collect()
}

/// A compact per-harness readiness line, e.g.
/// `ready claude · not-ready codex (not authenticated)`. `None` when the host
/// advertised no readiness. Display-only; readiness is heuristic and advisory.
/// The reason is untrusted peer text, so it is sanitized before rendering and a
/// reason that sanitizes to empty is dropped.
pub(super) fn readiness_summary(items: &[HarnessReadiness]) -> Option<String> {
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
pub(super) fn budget_summary(items: &[HarnessBudget]) -> Option<String> {
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
