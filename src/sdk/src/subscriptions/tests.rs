//! Unit tests for the pure halves of the subscription probes: the response
//! shapes each provider actually sends, and the timestamp parsing they need.

use serde_json::json;

use crate::config::{MeterId, ResourceDisplay, SubscriptionsConfig};

use super::time::parse_rfc3339;
use super::types::{MeterReading, SubscriptionSnapshot};
use super::{absorb, claude, codex, openrouter, tinyhumans};

/// A live Claude usage response, trimmed to the fields the probe reads.
fn claude_body() -> serde_json::Value {
    json!({
        "five_hour": {"utilization": 6.0, "resets_at": "2026-08-27T16:49:59.653759+00:00"},
        "seven_day": {"utilization": 40.0, "resets_at": "2026-08-29T22:59:59.653789+00:00"},
        "limits": [
            {"kind": "session", "percent": 6, "resets_at": "2026-08-27T16:49:59.653759+00:00"},
            {"kind": "weekly_all", "percent": 40, "resets_at": "2026-08-29T22:59:59.653789+00:00"},
            {"kind": "weekly_scoped", "percent": 12, "resets_at": null,
             "scope": {"model": {"display_name": "Fable"}}}
        ],
        "spend": {
            "used": {"amount_minor": 1250, "currency": "USD", "exponent": 2},
            "limit": {"amount_minor": 7000, "currency": "USD", "exponent": 2},
            "percent": 18
        }
    })
}

#[test]
fn claude_windows_become_meters() {
    let readings = claude::readings(&claude_body());
    let by_id = |id: MeterId| {
        readings
            .iter()
            .find(|(meter, _)| *meter == id)
            .map(|(_, reading)| reading.clone())
            .expect("meter present")
    };
    assert_eq!(by_id(MeterId::ClaudeSession).used_fraction, Some(0.06));
    assert_eq!(by_id(MeterId::ClaudeWeekly).used_fraction, Some(0.40));
    let scoped = by_id(MeterId::ClaudeScoped);
    assert_eq!(scoped.used_fraction, Some(0.12));
    assert_eq!(scoped.qualifier.as_deref(), Some("Fable"));
    let credits = by_id(MeterId::ClaudeCredits);
    assert_eq!(credits.remaining, Some(57.5));
    assert_eq!(credits.limit, Some(70.0));
}

#[test]
fn claude_falls_back_to_the_window_objects() {
    // A response without `limits` — the older shape — still yields the two
    // windows an operator hits, rather than nothing at all.
    let mut body = claude_body();
    body.as_object_mut().expect("object").remove("limits");
    let readings = claude::readings(&body);
    assert_eq!(readings.len(), 3, "two windows plus spend: {readings:?}");
    assert!(readings.iter().any(|(id, _)| *id == MeterId::ClaudeSession));
}

#[test]
fn codex_splits_its_windows_by_length() {
    let limits = json!({
        "primary": {"used_percent": 22.0, "window_minutes": 10080, "resets_at": 1788272085},
        "secondary": {"used_percent": 4.5, "window_minutes": 300, "resets_at": 1788200000}
    });
    let readings = codex::readings(&limits);
    let weekly = readings
        .iter()
        .find(|(id, _)| *id == MeterId::CodexWeekly)
        .expect("weekly");
    assert_eq!(weekly.1.used_fraction, Some(0.22));
    assert_eq!(weekly.1.resets_at, Some(1788272085));
    let session = readings
        .iter()
        .find(|(id, _)| *id == MeterId::CodexSession)
        .expect("session");
    assert_eq!(session.1.used_fraction, Some(0.045));
}

#[test]
fn openrouter_balance_is_purchased_less_used() {
    let reading = openrouter::reading(&json!({
        "data": {"total_credits": 50.0, "total_usage": 12.5}
    }))
    .expect("reading");
    assert_eq!(reading.remaining, Some(37.5));
    assert_eq!(reading.used_fraction, Some(0.25));
}

#[test]
fn portal_balance_measures_spend_against_the_cycle() {
    let reading = tinyhumans::reading(&json!({
        "remainingUsd": 30.0,
        "inferenceTotals": {"spent": 10.0}
    }))
    .expect("reading");
    assert_eq!(reading.remaining, Some(30.0));
    assert_eq!(reading.used_fraction, Some(0.25));
    // An account the runtime reports no balance for is not a broken reading —
    // there is simply nothing to meter.
    assert!(tinyhumans::reading(&json!({"plan": "team"})).is_none());
}

#[test]
fn a_failed_probe_marks_every_enabled_meter_of_its_source() {
    let mut config = SubscriptionsConfig::with_defaults();
    config.claude_session = ResourceDisplay::Bar;
    config.claude_weekly = ResourceDisplay::Percent;
    let mut meters = Default::default();
    absorb(
        &mut meters,
        &config,
        crate::config::MeterSource::Claude,
        Err("Claude login expired".into()),
    );
    assert_eq!(meters.len(), 2, "only the switched-on meters: {meters:?}");
    assert_eq!(
        meters[&MeterId::ClaudeSession].note.as_deref(),
        Some("Claude login expired")
    );
    // The scoped window was off, so it is absent rather than reported broken.
    assert!(!meters.contains_key(&MeterId::ClaudeScoped));
}

#[test]
fn an_answered_probe_still_accounts_for_a_meter_it_skipped() {
    let mut config = SubscriptionsConfig::with_defaults();
    config.claude_scoped = ResourceDisplay::Bar;
    let mut meters = Default::default();
    absorb(
        &mut meters,
        &config,
        crate::config::MeterSource::Claude,
        Ok(vec![(MeterId::ClaudeSession, MeterReading::default())]),
    );
    assert!(meters[&MeterId::ClaudeScoped].note.is_some());
}

#[test]
fn staleness_drives_the_refresh() {
    let snapshot = SubscriptionSnapshot {
        sampled_at: 1_000,
        ..SubscriptionSnapshot::default()
    };
    assert!(!snapshot.is_stale(1_100, 300));
    assert!(snapshot.is_stale(1_300, 300));
    // Never sampled: stale at any clock, which is what fetches on start-up.
    assert!(SubscriptionSnapshot::default().is_stale(0, 300));
}

#[test]
fn reset_timestamps_parse_across_the_shapes_providers_send() {
    assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(
        parse_rfc3339("2026-08-27T16:49:59.653759+00:00"),
        Some(1_787_849_399)
    );
    // An offset is applied, not ignored: the same instant, written two ways.
    assert_eq!(
        parse_rfc3339("2026-08-27T18:49:59+02:00"),
        parse_rfc3339("2026-08-27T16:49:59Z")
    );
    assert_eq!(parse_rfc3339("not a timestamp"), None);
    assert_eq!(parse_rfc3339("2026-13-01T00:00:00Z"), None);
}

#[test]
fn nothing_is_sampled_while_every_meter_is_off() {
    let config = SubscriptionsConfig::with_defaults();
    assert!(!config.any_enabled());
    let snapshot = futures::executor::block_on(super::sample(&config, None));
    assert!(snapshot.meters.is_empty());
}

/// Sample every meter against the real machine and the real providers.
///
/// Ignored by default: it reads the operator's own Claude login and OpenRouter
/// key and makes two network calls. Run it with `--ignored` to see what this
/// device actually reports.
#[test]
#[ignore = "live: reads local harness credentials and calls Claude/OpenRouter"]
fn live_sample_reports_this_machine() {
    let config = SubscriptionsConfig {
        claude_session: ResourceDisplay::Bar,
        claude_weekly: ResourceDisplay::Bar,
        claude_scoped: ResourceDisplay::Bar,
        claude_credits: ResourceDisplay::Value,
        codex_session: ResourceDisplay::Bar,
        codex_weekly: ResourceDisplay::Bar,
        open_router_credits: ResourceDisplay::Value,
        ..SubscriptionsConfig::with_defaults()
    };
    let snapshot = tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(super::sample(&config, None));
    for (meter, reading) in &snapshot.meters {
        println!("{:<20} {reading:?}", meter.label());
    }
    assert!(!snapshot.meters.is_empty());
}
