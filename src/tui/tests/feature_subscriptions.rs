//! Feature tests for the subscription-usage meters: the Settings subpage that
//! configures them, and the Agents sidebar footer that shows them.
//!
//! The meters are the one part of the sidebar whose settings reach off the
//! machine, so the coverage here is as much about what stays *off* — no meter
//! enabled, no line drawn, no sample requested — as about what renders.

use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

use medulla::config::{
    AppearanceConfig, LinkConfig, LoadedConfig, MeterId, ResourceDisplay, SubscriptionsConfig,
};
use medulla::runtime::mock::MockRuntime;
use medulla::subscriptions::{MeterReading, SubscriptionSnapshot};
use medulla_tui::ui::app::{App, Cmd, TABS};

fn loaded() -> LoadedConfig {
    let mut loaded = LoadedConfig::defaults("medulla.tui.json".into());
    loaded.config.link = Some(LinkConfig::default());
    loaded
}

fn key(app: &mut App, code: KeyCode) -> Option<Cmd> {
    app.on_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)))
}

fn draw(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    terminal.backend().buffer().clone()
}

fn text_of(buffer: &Buffer) -> String {
    buffer.content().iter().map(|cell| cell.symbol()).collect()
}

/// A fixed sample, so the meters say the same thing on every machine.
fn sample() -> SubscriptionSnapshot {
    let mut meters = std::collections::BTreeMap::new();
    meters.insert(
        MeterId::ClaudeSession,
        MeterReading {
            used_fraction: Some(0.06),
            resets_at: Some(4_000),
            ..MeterReading::default()
        },
    );
    meters.insert(
        MeterId::ClaudeWeekly,
        MeterReading {
            used_fraction: Some(0.40),
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
    meters.insert(
        MeterId::CodexWeekly,
        MeterReading::unavailable("no Codex sessions"),
    );
    SubscriptionSnapshot {
        meters,
        sampled_at: 1_000,
    }
}

/// The Settings tab, parked on the Subscriptions subpage.
fn settings_app(config: SubscriptionsConfig) -> App {
    let mut loaded = loaded();
    loaded.config.subscriptions = config;
    let mut app = App::new(Arc::new(MockRuntime::demo()), loaded);
    app.tab_index = TABS.iter().position(|tab| *tab == "Settings").unwrap();
    // Entering the page asks for a sample; delivering it is what clears the
    // in-flight flag, so the keys under test are not swallowed by the guard
    // that stops a refresh being queued behind itself.
    let _ = key(&mut app, KeyCode::Char('3'));
    let _ = app.set_subscriptions(sample());
    app
}

/// The Agents tab with `config` and the fixed sample already delivered.
fn agents_app(config: SubscriptionsConfig) -> App {
    agents_app_with(config, AppearanceConfig::default())
}

/// The same, with device indicators on too, so the shared footer can be seen
/// and asserted as one block rather than two that nearly line up.
fn agents_app_with(config: SubscriptionsConfig, appearance: AppearanceConfig) -> App {
    let mut loaded = loaded();
    loaded.config.subscriptions = config;
    loaded.config.appearance = appearance;
    let mut app = App::new(Arc::new(MockRuntime::demo()), loaded);
    let _ = app.set_subscriptions(sample());
    app.tab_index = TABS.iter().position(|tab| *tab == "Sessions").unwrap();
    app
}

#[test]
fn the_subpage_lists_every_meter_under_its_provider() {
    let mut app = settings_app(SubscriptionsConfig::default());
    assert_eq!(app.settings_subpage(), "Subscriptions");
    let out = text_of(&draw(&mut app, 160, 45));
    for label in [
        "Claude 5h",
        "Claude week",
        "Codex 5h",
        "OpenRouter",
        "TinyHumans",
    ] {
        assert!(out.contains(label), "missing meter {label}: {out}");
    }
    // Grouped by the subscription they read from, with the provenance stated:
    // a stale reading and a broken one look the same without it.
    assert!(
        out.contains("Read from the newest Codex transcript"),
        "provenance: {out}"
    );
}

#[test]
fn an_enabled_meter_shows_its_live_reading_beside_the_setting() {
    let mut app = settings_app(SubscriptionsConfig {
        claude_session: ResourceDisplay::Bar,
        open_router_credits: ResourceDisplay::Value,
        codex_weekly: ResourceDisplay::Percent,
        ..SubscriptionsConfig::default()
    });
    let out = text_of(&draw(&mut app, 160, 45));
    assert!(out.contains("6% used"), "the window's share: {out}");
    assert!(out.contains("$37.50 of $50.00 left"), "the balance: {out}");
    // A meter whose source could not answer says why, which is the thing the
    // sidebar has no room for.
    assert!(out.contains("no Codex sessions"), "the reason: {out}");
}

#[test]
fn cycling_a_meter_persists_the_section_and_samples_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut app = settings_app(SubscriptionsConfig::default());
    app.set_config_path(path.clone());

    // The first row is Claude's five-hour window; switching it on asks for a
    // sample rather than leaving the row blank until the next tick.
    let cmd = key(&mut app, KeyCode::Right);
    assert!(matches!(cmd, Some(Cmd::LoadSubscriptions(_))), "{cmd:?}");

    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("[subscriptions]"), "{saved}");
    assert!(saved.contains("claudeSession = \"percent\""), "{saved}");
    // Every other meter is written too, so a later load cannot silently reset
    // the ones this edit did not touch.
    assert!(saved.contains("codexWeekly = \"off\""), "{saved}");
    assert!(saved.contains("refreshSeconds = 300"), "{saved}");
}

#[test]
fn switching_a_meter_back_off_asks_for_nothing() {
    let mut app = settings_app(SubscriptionsConfig {
        claude_session: ResourceDisplay::Percent,
        ..SubscriptionsConfig::default()
    });
    // Left from `percent` is `off` — no sample, because nothing on this page is
    // now asking a provider anything.
    let cmd = key(&mut app, KeyCode::Left);
    assert!(cmd.is_none(), "{cmd:?}");
}

#[test]
fn r_resamples_on_demand() {
    let mut app = settings_app(SubscriptionsConfig {
        claude_session: ResourceDisplay::Bar,
        ..SubscriptionsConfig::default()
    });
    let cmd = key(&mut app, KeyCode::Char('r'));
    assert!(matches!(cmd, Some(Cmd::LoadSubscriptions(_))), "{cmd:?}");
}

#[test]
fn enabled_meters_render_in_the_agents_sidebar() {
    let mut app = agents_app(SubscriptionsConfig {
        claude_session: ResourceDisplay::Bar,
        claude_weekly: ResourceDisplay::Percent,
        open_router_credits: ResourceDisplay::Value,
        ..SubscriptionsConfig::default()
    });
    let out = text_of(&draw(&mut app, 140, 40));
    // Labels on the left, values against the rail's right border, sharing the
    // device readings' alignment above them.
    for (label, value) in [
        ("Claude 5h", "░░░░   6%│"),
        ("Claude week", " 40%│"),
        ("OpenRouter", "$37.5│"),
    ] {
        assert!(out.contains(label), "missing {label}: {out}");
        assert!(out.contains(value), "{label} is not flush right: {out}");
    }
    // Navigation survives the footer.
    assert!(out.contains("task-1"), "{out}");
}

#[test]
fn meters_stay_off_by_default() {
    let mut app = agents_app(SubscriptionsConfig::default());
    let out = text_of(&draw(&mut app, 140, 40));
    assert!(!out.contains("Claude 5h"), "{out}");
    assert!(!out.contains("OpenRouter"), "{out}");
    // And nothing asks a provider anything while they are all off.
    assert!(app.due_subscription_refresh().is_none());
}

#[test]
fn a_stale_sample_is_what_schedules_the_next_one() {
    let mut app = agents_app(SubscriptionsConfig {
        claude_session: ResourceDisplay::Bar,
        ..SubscriptionsConfig::default()
    });
    // The injected sample is dated 1970, so the first tick after it is due.
    let cmd = app.due_subscription_refresh();
    assert!(matches!(cmd, Some(Cmd::LoadSubscriptions(_))), "{cmd:?}");
    // …and a second tick does not queue a duplicate behind the one in flight.
    assert!(app.due_subscription_refresh().is_none());
}

#[test]
fn tinyhumans_meter_loads_account_usage_before_sampling() {
    let mut app = agents_app(SubscriptionsConfig {
        tinyhumans_balance: ResourceDisplay::Value,
        ..SubscriptionsConfig::default()
    });

    let cmd = app.due_subscription_refresh();
    assert!(matches!(cmd, Some(Cmd::LoadUsage)), "{cmd:?}");

    let account = serde_json::json!({"remainingUsd": 42.5});
    let cmd = app.set_account_usage(Some(account.clone()));
    assert!(
        matches!(cmd, Some(Cmd::LoadSubscriptions(Some(ref data))) if *data == account),
        "{cmd:?}"
    );
}

#[test]
fn loaded_account_usage_resamples_an_enabled_tinyhumans_meter() {
    let mut app = agents_app(SubscriptionsConfig {
        tinyhumans_balance: ResourceDisplay::Value,
        ..SubscriptionsConfig::default()
    });

    let cmd = app.set_account_usage(Some(serde_json::json!({"remainingUsd": 7.0})));
    assert!(
        matches!(cmd, Some(Cmd::LoadSubscriptions(Some(_)))),
        "{cmd:?}"
    );
}

#[test]
fn tinyhumans_refreshes_usage_even_after_a_previous_load() {
    let mut app = agents_app(SubscriptionsConfig {
        tinyhumans_balance: ResourceDisplay::Value,
        ..SubscriptionsConfig::default()
    });
    let _ = app.set_account_usage(Some(serde_json::json!({"remainingUsd": 7.0})));
    let _ = app.set_subscriptions(sample());

    let cmd = app.due_subscription_refresh();
    assert!(matches!(cmd, Some(Cmd::LoadUsage)), "{cmd:?}");
}

#[test]
fn failed_tinyhumans_usage_refresh_keeps_the_last_reading() {
    let mut app = agents_app(SubscriptionsConfig {
        tinyhumans_balance: ResourceDisplay::Value,
        ..SubscriptionsConfig::default()
    });
    let account = serde_json::json!({"remainingUsd": 7.0});
    let _ = app.set_account_usage(Some(account.clone()));
    let _ = app.set_subscriptions(sample());

    assert!(matches!(
        app.due_subscription_refresh(),
        Some(Cmd::LoadUsage)
    ));
    let cmd = app.usage_load_failed();
    assert!(
        matches!(cmd, Some(Cmd::LoadSubscriptions(Some(ref data))) if *data == account),
        "{cmd:?}"
    );
}

#[test]
fn a_setting_changed_during_sampling_queues_one_follow_up() {
    let mut app = settings_app(SubscriptionsConfig::default());

    let first = key(&mut app, KeyCode::Char('r'));
    assert!(
        matches!(first, Some(Cmd::LoadSubscriptions(_))),
        "{first:?}"
    );

    // The active command already cloned the all-off config. Enabling the first
    // row cannot start a second concurrent sample, but must be remembered.
    assert!(key(&mut app, KeyCode::Right).is_none());
    let follow_up = app.set_subscriptions(sample());
    assert!(
        matches!(follow_up, Some(Cmd::LoadSubscriptions(_))),
        "{follow_up:?}"
    );

    // Completing the follow-up drains the one-slot queue.
    assert!(app.set_subscriptions(sample()).is_none());
}

#[test]
fn automatic_sampling_clamps_too_short_configured_intervals() {
    let mut app = agents_app(SubscriptionsConfig {
        claude_session: ResourceDisplay::Percent,
        refresh_seconds: 0,
        ..SubscriptionsConfig::default()
    });
    let now = medulla::clock::now_millis() / 1_000;
    let _ = app.set_subscriptions(SubscriptionSnapshot {
        sampled_at: now.saturating_sub(1),
        ..SubscriptionSnapshot::default()
    });

    assert!(app.due_subscription_refresh().is_none());
}

/// Print the sidebar with every meter on, for eyeballing the footer's layout.
///
/// Ignored: it asserts nothing the other tests do not, and exists so a change
/// to the footer can be looked at rather than only diffed.
#[test]
#[ignore = "visual: prints the Agents sidebar with every meter enabled"]
fn visual_sidebar_with_every_meter() {
    let mut app = agents_app_with(
        SubscriptionsConfig {
            claude_session: ResourceDisplay::Bar,
            claude_weekly: ResourceDisplay::Bar,
            open_router_credits: ResourceDisplay::Value,
            codex_weekly: ResourceDisplay::Bar,
            ..SubscriptionsConfig::default()
        },
        AppearanceConfig {
            device_cpu: ResourceDisplay::Bar,
            device_ram: ResourceDisplay::Bar,
            ..AppearanceConfig::default()
        },
    );
    let buffer = draw(&mut app, 120, 30);
    for row in 0..buffer.area.height {
        let line: String = (0..40)
            .map(|column| buffer[(column, row)].symbol())
            .collect();
        println!("|{line}|");
    }
}

/// Print the Subscriptions settings page, for the same reason.
#[test]
#[ignore = "visual: prints the Subscriptions settings subpage"]
fn visual_subscriptions_page() {
    let mut app = settings_app(SubscriptionsConfig {
        claude_session: ResourceDisplay::Bar,
        claude_weekly: ResourceDisplay::Percent,
        open_router_credits: ResourceDisplay::Value,
        codex_weekly: ResourceDisplay::Bar,
        ..SubscriptionsConfig::default()
    });
    let buffer = draw(&mut app, 140, 44);
    for row in 0..buffer.area.height {
        let line: String = (0..buffer.area.width)
            .map(|column| buffer[(column, row)].symbol())
            .collect();
        println!("|{}|", line.trim_end());
    }
}

#[test]
fn the_device_and_meter_blocks_share_one_right_edge() {
    let mut app = agents_app_with(
        SubscriptionsConfig {
            claude_session: ResourceDisplay::Bar,
            ..SubscriptionsConfig::default()
        },
        AppearanceConfig {
            device_cpu: ResourceDisplay::Bar,
            ..AppearanceConfig::default()
        },
    );
    app.set_device_snapshot(medulla_tui::ui::resources::DeviceSnapshot {
        cpu_fraction: Some(0.42),
        ..Default::default()
    });
    let out = text_of(&draw(&mut app, 140, 40));
    // One footer, one layout: both blocks put their bar and percentage in the
    // same columns against the rail's border.
    assert!(out.contains("██░░  42%│"), "device: {out}");
    assert!(out.contains("░░░░   6%│"), "meter: {out}");
}
