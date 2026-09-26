//! Responsive formatting of subscription meters into sidebar lines.
//!
//! Shares its layout with the device readings above it in the footer — labels
//! flush left, values flush right, fixed-width bars and percentages — through
//! [`crate::ui::gauge`].

use medulla::config::{MeterId, ResourceDisplay, SubscriptionsConfig};
use medulla::subscriptions::{MeterReading, SubscriptionSnapshot};

use crate::ui::gauge::{bar_value, percent, Gauge};

/// Shown in place of a reading no provider could supply.
const UNAVAILABLE: &str = "—";

/// Format the switched-on meters, in the order [`MeterId::ALL`] declares.
///
/// `max_lines` is the budget the sidebar can spare without eating navigation
/// rows, and `width` its inner width; a narrow rail degrades bars and amounts to
/// percentages rather than overflowing. A meter that is on but has no reading
/// still emits a line — a meter that vanished when its provider went quiet would
/// read as "nothing to report", which is the wrong claim.
pub fn subscription_lines(
    config: &SubscriptionsConfig,
    snapshot: &SubscriptionSnapshot,
    width: usize,
    max_lines: usize,
) -> Vec<String> {
    meter_gauges(config, snapshot, width)
        .into_iter()
        .take(max_lines)
        .map(|gauge| gauge.render(width))
        .collect()
}

/// The switched-on meters as gauges, already degraded to what `width` holds.
fn meter_gauges(
    config: &SubscriptionsConfig,
    snapshot: &SubscriptionSnapshot,
    width: usize,
) -> Vec<Gauge> {
    MeterId::ALL
        .into_iter()
        .filter_map(|meter| meter_gauge(meter, config.display(meter), snapshot.get(meter), width))
        .collect()
}

/// Render one meter, or `None` when it is switched off.
fn meter_gauge(
    meter: MeterId,
    display: ResourceDisplay,
    reading: Option<&MeterReading>,
    width: usize,
) -> Option<Gauge> {
    if display == ResourceDisplay::Off {
        return None;
    }
    let label = label(meter, reading);
    let Some(reading) = reading.filter(|reading| reading.has_value()) else {
        return Some(Gauge::new(label, UNAVAILABLE));
    };
    let percent = reading
        .used_fraction
        .map(|fraction| Gauge::new(label.clone(), percent(fraction)));
    let amount = reading
        .remaining
        .map(|remaining| Gauge::new(label.clone(), money(remaining, reading.currency.as_deref())));
    // Whichever of the two the provider gave, when the requested form is not
    // the one it supports: a balance with no cap has no percentage, and a
    // window has no amount. Falling back beats printing an em dash over a
    // number the operator can see on the settings page.
    let fallback = percent.clone().or_else(|| amount.clone());
    let gauge = match display {
        ResourceDisplay::Off => return None,
        ResourceDisplay::Percent => fallback?,
        ResourceDisplay::Value => amount.or(percent)?,
        ResourceDisplay::Bar => match reading.used_fraction {
            Some(fraction) => {
                let bar = Gauge::new(label, bar_value(fraction));
                if bar.natural_width() <= width {
                    bar
                } else {
                    fallback?
                }
            }
            // No fraction to draw a bar against.
            None => fallback?,
        },
    };
    Some(gauge)
}

/// The meter's label, refined by whatever the provider called this window.
///
/// Claude's per-model weekly limit is the case: "Claude model" says nothing an
/// operator can act on, while "Claude Fable" is the row they were looking for.
fn label(meter: MeterId, reading: Option<&MeterReading>) -> String {
    let qualifier = reading.and_then(|reading| reading.qualifier.as_deref());
    match (meter, qualifier) {
        (MeterId::ClaudeScoped, Some(model)) => format!("Claude {model}"),
        _ => meter.label().to_string(),
    }
}

/// A dollar amount, at the precision the magnitude deserves.
///
/// Cents matter on a balance that is nearly out and are noise on one that is
/// not, and the rail is too narrow to spend four columns saying `.00`.
fn money(value: f64, currency: Option<&str>) -> String {
    let prefix = match currency.unwrap_or("USD") {
        "USD" => "$",
        "EUR" => "€",
        "GBP" => "£",
        code => return format!("{code} {value:.2}"),
    };
    if value >= 100.0 {
        format!("{prefix}{value:.0}")
    } else if value >= 10.0 {
        format!("{prefix}{value:.1}")
    } else {
        format!("{prefix}{value:.2}")
    }
}

/// Columns the switched-on meters need to render in their configured form.
///
/// Bars and percentages are fixed width, so unlike the device readings' byte
/// pairs these do not have to be measured against a saturated copy of the
/// snapshot: a meter at 6% asks for exactly what the same meter at 100% will.
pub fn subscription_width_hint(
    config: &SubscriptionsConfig,
    snapshot: &SubscriptionSnapshot,
) -> usize {
    // Measured against an unbounded rail, so the hint reports what the meters
    // want rather than what a rail that is currently too narrow allows.
    meter_gauges(config, snapshot, usize::MAX)
        .into_iter()
        .map(|gauge| gauge.natural_width())
        .max()
        .unwrap_or(0)
}
