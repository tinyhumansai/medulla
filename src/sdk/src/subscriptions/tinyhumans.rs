//! The TinyHumans portal balance, taken from the account usage already loaded.
//!
//! Settings → Usage fetches this document for its own page, so the meter reads
//! from that rather than issuing a second call: one balance, one request, and a
//! refresh on either surface updates both.

use serde_json::Value;

use super::types::MeterReading;

/// Map an account-usage document onto the balance reading.
///
/// `None` when the document carries no balance at all — a runtime with no
/// billing attached, or a response shape this build does not know.
pub fn reading(usage: &Value) -> Option<MeterReading> {
    let remaining = usage.get("remainingUsd").and_then(Value::as_f64);
    let spent = usage
        .pointer("/inferenceTotals/spent")
        .and_then(Value::as_f64);
    remaining?;
    // The portal states what is left and what has been spent this cycle, not a
    // cap. Their sum is the allowance the cycle started with, which is what a
    // bar has to be drawn against.
    let allowance = match (remaining, spent) {
        (Some(remaining), Some(spent)) if remaining + spent > 0.0 => Some(remaining + spent),
        _ => None,
    };
    Some(MeterReading {
        used_fraction: match (allowance, spent) {
            (Some(allowance), Some(spent)) => Some((spent / allowance).clamp(0.0, 1.0)),
            _ => None,
        },
        remaining,
        limit: allowance,
        currency: Some("USD".into()),
        ..MeterReading::default()
    })
}
