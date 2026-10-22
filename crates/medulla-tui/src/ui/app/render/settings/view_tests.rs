//! Deterministic Settings view tests. The page renderers accept their data so
//! these tests never inspect the host or sample a provider account.

use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

use super::{harnesses, tokens};
use crate::ui::app::types::{DetectedProvider, SETTINGS_GROUPS, SETTINGS_SUBPAGES};

fn rendered(draw: impl FnOnce(&mut ratatui::Frame<'_>, Rect)) -> String {
    let mut terminal = Terminal::new(TestBackend::new(100, 32)).expect("test terminal");
    terminal
        .draw(|frame| {
            let area = frame.area();
            draw(frame, area);
        })
        .expect("render view");
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>()
}

#[test]
fn tokens_and_harnesses_are_general_settings_sections() {
    let tokens = SETTINGS_SUBPAGES
        .iter()
        .position(|name| *name == "Tokens")
        .expect("Tokens Settings section");
    let harnesses = SETTINGS_SUBPAGES
        .iter()
        .position(|name| *name == "Harnesses")
        .expect("Harnesses Settings section");
    assert!(tokens < SETTINGS_GROUPS[1].1);
    assert!(harnesses < SETTINGS_GROUPS[1].1);
    assert_eq!(SETTINGS_GROUPS[0].0, "GENERAL");
    assert_eq!(SETTINGS_GROUPS[1].0, "DEBUG");
    assert_eq!(SETTINGS_GROUPS[2].0, "ABOUT");
}

#[test]
fn tokens_view_renders_categories_pricing_and_injected_subscription_meters() {
    use medulla::config::MeterId;
    use medulla::protocol::HarnessProvider;
    use medulla::subscriptions::{MeterReading, SubscriptionSnapshot};
    use medulla::ui::events::Usage;
    use std::collections::BTreeMap;

    let mut meters = BTreeMap::new();
    meters.insert(
        MeterId::ClaudeSession,
        MeterReading {
            used_fraction: Some(0.42),
            ..Default::default()
        },
    );
    let view = tokens::TokensView {
        rows: vec![
            tokens::TokenUsageRow {
                session: "session-1".into(),
                tier: "reasoning".into(),
                provider: Some(HarnessProvider::Claude),
                model: Some("claude-sonnet-4-6".into()),
                usage: Usage {
                    input_tokens: 1_000_000,
                    output_tokens: 200_000,
                    cache_read_tokens: Some(100_000),
                    cache_write_tokens: Some(100_000),
                    reasoning_tokens: Some(50_000),
                    ..Default::default()
                },
            },
            tokens::TokenUsageRow {
                session: "session-2".into(),
                tier: "unknown".into(),
                provider: Some(HarnessProvider::Claude),
                model: Some("not-in-pricing-table".into()),
                usage: Usage {
                    input_tokens: 2,
                    output_tokens: 1,
                    ..Default::default()
                },
            },
        ],
        subscriptions: SubscriptionSnapshot {
            meters,
            sampled_at: 1,
        },
        compression: Some(medulla::juice::EmbeddedCompressionStats {
            events: 2,
            tokens: medulla::juice::CompressionTokenTotals {
                before_tokens: 1200,
                after_tokens: 300,
            },
        }),
    };
    let output = rendered(|frame, area| tokens::draw_tokens(frame, area, &view));

    for expected in [
        "Tokens",
        "Local embedded workspace compression",
        "1200 estimated tokens before",
        "300 after",
        "900 saved",
        "2 compression events",
        "session-1",
        "Claude Code",
        "claude-sonnet-4-6",
        "input 1000000",
        "output 200000",
        "cache read 100000",
        "cache write 100000",
        "reasoning 50000",
        "cost $6.4050",
        "Claude 5h",
        "42%",
        "not-in-pricing-table",
        "cost unavailable",
    ] {
        assert!(
            output.contains(expected),
            "missing {expected:?} in:\n{output}"
        );
    }
}

#[test]
fn tokens_app_renders_pricing_only_for_provider_attributed_events() {
    use crate::ui::app::types::{tab_pos, App, SETTINGS_SUBPAGES};
    use medulla::config::LoadedConfig;
    use medulla::runtime::mock::MockRuntime;
    use medulla::runtime::Runtime;
    use std::sync::Arc;

    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut app = App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()));
    app.snapshot.session_id = "runtime-session-42".into();
    app.snapshot.events = vec![
        serde_json::from_value(serde_json::json!({
            "seq": 1,
            "at": 1,
            "event": {
                "kind": "inference_end",
                "tier": "reasoning",
                "op": "execute_step",
                "model": "claude-sonnet-4-6",
                "provider": "claude",
                "durationMs": 1,
                "usage": {
                    "inputTokens": 1_000_000,
                    "outputTokens": 200_000,
                    "cacheReadTokens": 100_000,
                    "cacheWriteTokens": 100_000,
                    "reasoningTokens": 50_000
                }
            }
        }))
        .unwrap(),
        serde_json::from_value(serde_json::json!({
            "seq": 2,
            "at": 2,
            "event": {
                "kind": "inference_end",
                "tier": "reasoning",
                "op": "execute_step",
                "model": "claude-sonnet-4-6",
                "durationMs": 1,
                "usage": { "inputTokens": 1_000_000, "outputTokens": 200_000 }
            }
        }))
        .unwrap(),
    ];
    app.tab_index = tab_pos("Settings");
    app.settings_index = SETTINGS_SUBPAGES
        .iter()
        .position(|name| *name == "Tokens")
        .unwrap();

    let output = rendered(|frame, _| app.draw(frame));
    for expected in [
        "runtime-session-42",
        "Claude Code",
        "claude-sonnet-4-6",
        "cost $6.4050",
        "unknown provider",
        "cost unavailable",
    ] {
        assert!(
            output.contains(expected),
            "missing {expected:?} in:\n{output}"
        );
    }
    assert_eq!(
        output.matches("input 1000000").count(),
        2,
        "provider-attributed and legacy usage must remain separate rows:\n{output}"
    );
}

#[test]
fn harnesses_view_renders_injected_detection_routing_budget_and_customizations() {
    let view = harnesses::HarnessesView {
        providers: vec![
            DetectedProvider {
                name: "Claude Code".into(),
                version: Some("2.1.4".into()),
            },
            DetectedProvider {
                name: "Codex".into(),
                version: Some("0.123.0".into()),
            },
        ],
        detection_complete: true,
        routing: vec![("reasoning".into(), "claude-sonnet-4-6".into())],
        budgets: vec![("claude".into(), "used 12000 / limit 50000".into())],
        configuration: vec![
            "hooks: 2 configured".into(),
            "Codex overrides configured for 1 preset(s)".into(),
            "attribution: commits enabled".into(),
            "MCP: fleet tools enabled".into(),
        ],
        live_facts: vec!["Managed skills installed: 3".into()],
    };
    let output = rendered(|frame, area| harnesses::draw_harnesses(frame, area, &view));

    for expected in [
        "Harnesses",
        "Claude Code",
        "2.1.4",
        "Codex",
        "0.123.0",
        "reasoning",
        "claude-sonnet-4-6",
        "used 12000 / limit 50000",
        "hooks: 2 configured",
        "Codex overrides configured for 1 preset(s)",
        "attribution: commits enabled",
        "MCP: fleet tools enabled",
        "Managed skills installed: 3",
    ] {
        assert!(
            output.contains(expected),
            "missing {expected:?} in:\n{output}"
        );
    }
}

#[test]
fn harnesses_view_explains_empty_detection_without_host_probes() {
    let view = harnesses::HarnessesView {
        detection_complete: true,
        ..Default::default()
    };
    let output = rendered(|frame, area| harnesses::draw_harnesses(frame, area, &view));
    assert!(output.contains("No providers detected"));
}

#[test]
fn selected_harnesses_page_uses_the_collected_app_snapshot() {
    use crate::ui::app::types::{tab_pos, App, HarnessesView};
    use medulla::config::LoadedConfig;
    use medulla::runtime::mock::MockRuntime;
    use medulla::runtime::Runtime;
    use std::sync::Arc;

    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut app = App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()));
    app.tab_index = tab_pos("Settings");
    app.settings_index = SETTINGS_SUBPAGES
        .iter()
        .position(|name| *name == "Harnesses")
        .unwrap();
    app.set_harnesses_view(HarnessesView {
        providers: vec![DetectedProvider {
            name: "Codex".into(),
            version: None,
        }],
        detection_complete: true,
        routing: vec![("reasoning".into(), "routed-model".into())],
        budgets: vec![("codex".into(), "remaining 9000".into())],
        configuration: vec!["Codex overrides configured for 1 preset(s)".into()],
        live_facts: vec!["Managed skills installed: 3".into()],
    });

    let output = rendered(|frame, _| app.draw(frame));
    for expected in [
        "Codex",
        "version unavailable",
        "routed-model",
        "remaining 9000",
        "Codex overrides configured for 1 preset(s)",
        "Managed skills installed: 3",
    ] {
        assert!(
            output.contains(expected),
            "missing {expected:?} in:\n{output}"
        );
    }
}

#[test]
fn entering_harnesses_queues_local_collection_with_loaded_sources() {
    use crate::ui::app::types::{tab_pos, App, Cmd};
    use medulla::config::LoadedConfig;
    use medulla::runtime::mock::MockRuntime;
    use medulla::runtime::Runtime;
    use std::sync::Arc;

    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut loaded = LoadedConfig::defaults("medulla.tui.json".into());
    loaded.sources = vec!["user-config.json".into(), "project-config.json".into()];
    let mut app = App::new(runtime, loaded);
    app.tab_index = tab_pos("Settings");
    app.settings_index = SETTINGS_SUBPAGES
        .iter()
        .position(|name| *name == "Harnesses")
        .unwrap();

    assert!(matches!(
        app.tab_enter_cmd(),
        Some(Cmd::LoadHarnesses { sources, .. })
            if sources
                == vec![
                    "user-config.json".to_string(),
                    "project-config.json".to_string()
                ]
    ));
}

#[test]
fn settings_dispatch_renders_both_new_sections_by_selection() {
    use crate::ui::app::types::{tab_pos, App};
    use medulla::config::LoadedConfig;
    use medulla::runtime::mock::MockRuntime;
    use medulla::runtime::Runtime;
    use std::sync::Arc;

    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut app = App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()));
    app.tab_index = tab_pos("Settings");
    let tokens = SETTINGS_SUBPAGES
        .iter()
        .position(|name| *name == "Tokens")
        .unwrap();
    app.settings_index = tokens;
    let output = rendered(|frame, _| app.draw(frame));
    assert!(
        output.contains("Tokens"),
        "selected Tokens page was not drawn"
    );

    let harnesses = SETTINGS_SUBPAGES
        .iter()
        .position(|name| *name == "Harnesses")
        .unwrap();
    app.settings_index = harnesses;
    let output = rendered(|frame, _| app.draw(frame));
    assert!(
        output.contains("Harnesses"),
        "selected Harnesses page was not drawn"
    );
}

#[test]
fn existing_usage_page_still_uses_the_legacy_input_output_presentation() {
    use crate::ui::app::types::SETTINGS_SUBPAGES;
    use crate::ui::app::App;
    use medulla::config::LoadedConfig;
    use medulla::runtime::mock::MockRuntime;
    use medulla::runtime::Runtime;
    use std::sync::Arc;

    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut app = App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()));
    app.settings_index = SETTINGS_SUBPAGES
        .iter()
        .position(|name| *name == "Usage")
        .unwrap();
    let output = rendered(|frame, area| app.draw_settings(frame, area));
    assert!(output.contains("This session"));
    assert!(output.contains("Account"));
    assert!(!output.contains("cache read"));
    assert!(!output.contains("reasoning"));
}

#[test]
fn tokens_view_explains_absent_embedded_compression_without_invented_zeroes() {
    let output =
        rendered(|frame, area| tokens::draw_tokens(frame, area, &tokens::TokensView::default()));
    for expected in [
        "Local embedded workspace compression",
        "No embedded compression savings reported.",
        "Claude/Codex hooks report usage but do not compress tool output.",
    ] {
        assert!(
            output.contains(expected),
            "missing {expected:?} in:\n{output}"
        );
    }
    assert!(!output.contains("0 saved"));
    assert!(!output.contains("bytes saved"));
    assert!(!output.contains("cost saved"));
}
