//! Compact account-usage formatting for token windows and decimal balances.

use std::collections::BTreeMap;

use medulla::runtime::HarnessBudget;

/// Fold repeated host reports into one conservative summary per usage window.
pub(super) fn usage_summary(budgets: &[&HarnessBudget]) -> Option<String> {
    let mut by_window: BTreeMap<String, &HarnessBudget> = BTreeMap::new();
    for budget in budgets {
        let key = budget.window.trim().to_ascii_lowercase();
        let entry = by_window.entry(key).or_insert(*budget);
        if tighter_than(budget, entry) {
            *entry = budget;
        }
    }

    let segments: Vec<String> = by_window
        .values()
        .filter_map(|budget| usage_segment(budget))
        .collect();
    (!segments.is_empty()).then(|| segments.join(" | "))
}

/// Prefer the lower known headroom when hosts report the same account/window.
fn tighter_than(candidate: &HarnessBudget, current: &HarnessBudget) -> bool {
    match (candidate.fraction_remaining(), current.fraction_remaining()) {
        (Some(candidate), Some(current)) => candidate < current,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => match (candidate.remaining(), current.remaining()) {
            (Some(candidate), Some(current)) => candidate < current,
            (Some(_), None) => true,
            _ => false,
        },
    }
}

/// Format one account window when it contains a usable headroom signal.
fn usage_segment(budget: &HarnessBudget) -> Option<String> {
    let label = match budget.window.trim() {
        "" | "unknown" | "window unknown" => "balance",
        window => window,
    };
    let mut segment = if let Some(remaining) = budget.remaining_amount() {
        format!(
            "{label} · {} left",
            amount(remaining, budget.amount_unit.as_deref())
        )
    } else if let Some(remaining) = budget.remaining() {
        format!("{label} · {} tokens left", tokens(remaining))
    } else if budget.cooldown_until.is_some() {
        label.to_string()
    } else {
        return None;
    };

    if let Some(fraction) = budget.fraction_remaining() {
        segment.push_str(&format!(" ({:.0}%)", fraction * 100.0));
    }
    if let Some(until) = budget.cooldown_until {
        segment.push_str(&format!(" · cooldown {until}"));
    }
    Some(segment)
}

/// Format a decimal provider balance, with currency-aware USD output.
fn amount(value: f64, unit: Option<&str>) -> String {
    match unit.map(str::trim).filter(|unit| !unit.is_empty()) {
        Some(unit) if unit.eq_ignore_ascii_case("usd") => format!("${value:.2}"),
        Some(unit) => format!("{value:.2} {unit}"),
        None => format!("{value:.2}"),
    }
}

/// Scale token headroom without overstating fractional thousands.
fn tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
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
