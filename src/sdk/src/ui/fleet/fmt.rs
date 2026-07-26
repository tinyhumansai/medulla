//! Small formatters shared by the fleet rows and the detail pane: byte and
//! token magnitudes, budget windows, and availability wording.
//!
//! Numbers here are deliberately coarse — whole or one-decimal GB, `250k`
//! rather than `250,431` — for the same reason the library's own capacity block
//! is: an operator reads a fleet view for shape, and a figure that jitters on
//! every refresh is harder to scan, not more informative.

use crate::runtime::fleet::{HarnessBudget, HostResources};

/// A byte count as GB with one decimal, or an empty string when absent.
pub fn gb(bytes: Option<u64>) -> String {
    match bytes {
        Some(b) => format!("{:.1}GB", b as f64 / 1_073_741_824.0),
        None => String::new(),
    }
}

/// A token magnitude: `980`, `12k`, `1.2M`.
pub fn tokens(n: u64) -> String {
    if n < 1_000 {
        return n.to_string();
    }
    if n < 1_000_000 {
        return format!("{}k", (n as f64 / 1_000.0).round() as u64);
    }
    format!("{:.1}M", n as f64 / 1_000_000.0)
}

/// A host's declared resources as one line: `8 cores · 12.0GB / 32.0GB free`.
/// Empty when the host declared nothing.
pub fn resources(resources: Option<&HostResources>) -> String {
    let Some(r) = resources else {
        return String::new();
    };
    let mut parts = Vec::new();
    if let Some(cores) = r.cpu_cores {
        parts.push(if cores.fract() == 0.0 {
            format!("{} cores", cores as i64)
        } else {
            format!("{cores:.1} cores")
        });
    }
    match (r.available_memory_bytes, r.total_memory_bytes) {
        (Some(_), Some(_)) => parts.push(format!(
            "{} / {} free",
            gb(r.available_memory_bytes),
            gb(r.total_memory_bytes)
        )),
        (Some(_), None) => parts.push(format!("{} free", gb(r.available_memory_bytes))),
        (None, Some(_)) => parts.push(gb(r.total_memory_bytes)),
        (None, None) => {}
    }
    if r.disk_free_bytes.is_some() {
        parts.push(format!("{} disk", gb(r.disk_free_bytes)));
    }
    parts.join(" · ")
}

/// One budget window as `anthropic 5h · 250k/1.0M left`, degrading to whatever
/// the provider actually reported.
///
/// A budget with neither a remainder nor a limit still renders its provider and
/// window: knowing a seat exists is worth a row even when its numbers are not.
pub fn budget(budget: &HarnessBudget) -> String {
    let head = format!("{} {}", budget.provider, budget.window);
    let amounts = match (budget.remaining(), budget.limit_tokens) {
        (Some(remaining), Some(limit)) => {
            format!("{}/{} left", tokens(remaining), tokens(limit))
        }
        (Some(remaining), None) => format!("{} left", tokens(remaining)),
        (None, Some(limit)) => format!("{} limit", tokens(limit)),
        (None, None) => String::new(),
    };
    let mut out = head;
    if !amounts.is_empty() {
        out = format!("{out} · {amounts}");
    }
    if budget.cooldown_until.is_some() {
        out = format!("{out} · cooling down");
    }
    out
}

/// The tightest budget across a harness's windows, which is the one that will
/// actually stop work. `None` when no window reports a usable fraction.
pub fn tightest(budgets: &[HarnessBudget]) -> Option<&HarnessBudget> {
    budgets
        .iter()
        .filter(|b| b.fraction_remaining().is_some())
        .min_by(|a, b| {
            a.fraction_remaining()
                .unwrap()
                .total_cmp(&b.fraction_remaining().unwrap())
        })
}

/// Whether an open-vocabulary availability string means "cannot take work".
///
/// Only `offline` is a hard gate in medulla-v1; everything else — including
/// values this client has never seen — is treated as usable, so an unfamiliar
/// vocabulary degrades to "shown as available" rather than to "hidden".
pub fn is_offline(availability: &str) -> bool {
    availability.eq_ignore_ascii_case("offline")
}

/// Join non-empty parts with the fleet view's separator.
pub fn join(parts: &[String]) -> String {
    parts
        .iter()
        .filter(|p| !p.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" · ")
}
