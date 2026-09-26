//! Formatting tests: what each display setting renders, and how a narrow rail
//! degrades it.

use medulla::config::{MeterId, ResourceDisplay, SubscriptionsConfig};
use medulla::subscriptions::{MeterReading, SubscriptionSnapshot};

use super::{subscription_lines, subscription_width_hint};

/// The line one meter renders to: label flush left, value flush right.
///
/// The gap is whatever the rail width leaves, so the assertions pin the *edges*
/// rather than a particular number of spaces — which is the property the shared
/// footer layout actually promises.
fn row(label: &str, value: &str, width: usize) -> String {
    let gap = width - label.chars().count() - value.chars().count();
    format!("{label}{}{value}", " ".repeat(gap))
}

/// A snapshot with one Claude window and one OpenRouter balance.
fn snapshot() -> SubscriptionSnapshot {
    let mut meters = std::collections::BTreeMap::new();
    meters.insert(
        MeterId::ClaudeSession,
        MeterReading {
            used_fraction: Some(0.06),
            ..MeterReading::default()
        },
    );
    meters.insert(
        MeterId::ClaudeScoped,
        MeterReading {
            used_fraction: Some(0.5),
            qualifier: Some("Fable".into()),
            ..MeterReading::default()
        },
    );
    meters.insert(
        MeterId::OpenRouterCredits,
        MeterReading {
            used_fraction: Some(0.25),
            remaining: Some(37.5),
            limit: Some(50.0),
            currency: Some("USD".into()),
            ..MeterReading::default()
        },
    );
    SubscriptionSnapshot {
        meters,
        sampled_at: 1_000,
    }
}

#[test]
fn only_switched_on_meters_render() {
    let config = SubscriptionsConfig::with_defaults();
    assert!(subscription_lines(&config, &snapshot(), 40, 8).is_empty());
}

#[test]
fn each_display_renders_its_own_form() {
    let mut config = SubscriptionsConfig::with_defaults();
    config.claude_session = ResourceDisplay::Percent;
    config.open_router_credits = ResourceDisplay::Value;
    assert_eq!(
        subscription_lines(&config, &snapshot(), 40, 8),
        [row("Claude 5h", "  6%", 40), row("OpenRouter", "$37.5", 40),]
    );
    config.claude_session = ResourceDisplay::Bar;
    assert_eq!(
        subscription_lines(&config, &snapshot(), 40, 8)[0],
        row("Claude 5h", "░░░░   6%", 40)
    );
}

#[test]
fn a_scoped_window_is_labelled_by_its_model() {
    let mut config = SubscriptionsConfig::with_defaults();
    config.claude_scoped = ResourceDisplay::Bar;
    assert_eq!(
        subscription_lines(&config, &snapshot(), 40, 8),
        [row("Claude Fable", "██░░  50%", 40)]
    );
}

#[test]
fn a_narrow_rail_degrades_a_bar_to_its_percentage() {
    let mut config = SubscriptionsConfig::with_defaults();
    config.claude_session = ResourceDisplay::Bar;
    // Twelve columns is one short of the aligned `  6%`, so the percentage
    // drops its own padding rather than the rail losing the reading.
    assert_eq!(
        subscription_lines(&config, &snapshot(), 12, 8),
        ["Claude 5h 6%"]
    );
}

#[test]
fn a_meter_with_no_reading_still_says_so() {
    let mut config = SubscriptionsConfig::with_defaults();
    config.codex_weekly = ResourceDisplay::Bar;
    let mut snapshot = snapshot();
    snapshot.meters.insert(
        MeterId::CodexWeekly,
        MeterReading::unavailable("no Codex sessions"),
    );
    assert_eq!(
        subscription_lines(&config, &snapshot, 40, 8),
        [row("Codex week", "—", 40)]
    );
}

#[test]
fn the_line_budget_is_respected() {
    let mut config = SubscriptionsConfig::with_defaults();
    config.claude_session = ResourceDisplay::Percent;
    config.claude_scoped = ResourceDisplay::Percent;
    config.open_router_credits = ResourceDisplay::Percent;
    assert_eq!(subscription_lines(&config, &snapshot(), 40, 2).len(), 2);
}

#[test]
fn the_width_hint_sizes_the_rail_for_a_full_meter() {
    let mut config = SubscriptionsConfig::with_defaults();
    config.claude_session = ResourceDisplay::Bar;
    // A fixed-width percentage means the meter asks for the same 19 columns at
    // 6% as at 100%, so the bar never shuffles sideways as the window fills.
    assert_eq!(
        subscription_width_hint(&config, &snapshot()),
        "Claude 5h ████ 100%".chars().count()
    );
    assert_eq!(
        subscription_width_hint(&SubscriptionsConfig::with_defaults(), &snapshot()),
        0
    );
}
