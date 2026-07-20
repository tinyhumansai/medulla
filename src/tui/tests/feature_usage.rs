//! Feature tests for the Usage tab: the per-tier/per-task token fold over the
//! live event stream, the account panel, and the `/config` detail view.

use std::sync::Arc;

use ratatui::backend::TestBackend;
use ratatui::Terminal;
use serde_json::json;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use medulla::config::{LoadedConfig, TinyplaceConfig};
use medulla::runtime::mock::MockRuntime;
use medulla_tui::ui::app::{App, TABS};
use medulla_tui::ui::events::{TaskDigest, TuiEvent, Usage};

fn loaded() -> LoadedConfig {
    let mut l = LoadedConfig::defaults("medulla.tui.json".into());
    l.config.tinyplace = Some(TinyplaceConfig::default());
    l
}

fn usage_app() -> (App, Arc<MockRuntime>) {
    let rt = Arc::new(MockRuntime::empty());
    let mut app = App::new(rt.clone(), loaded());
    // Usage now lives as the default subpage of the Settings tab.
    app.tab_index = TABS.iter().position(|t| *t == "Settings").unwrap();
    (app, rt)
}

fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    buf.content().iter().map(|c| c.symbol()).collect()
}

fn key(app: &mut App, code: KeyCode) -> Option<medulla_tui::ui::app::Cmd> {
    app.on_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)))
}

fn script_usage(rt: &Arc<MockRuntime>) {
    rt.script_event(TuiEvent::InferenceStart {
        tier: "orchestrator".into(),
        op: "decide".into(),
        model: Some("medulla-v1".into()),
    });
    rt.script_event(TuiEvent::InferenceEnd {
        tier: "orchestrator".into(),
        op: "decide".into(),
        model: Some("medulla-v1".into()),
        duration_ms: 10,
        usage: Some(Usage {
            input_tokens: 1200,
            output_tokens: 340,
        }),
        content: None,
        reasoning: None,
        tool_calls: None,
    });
    rt.script_event(TuiEvent::InferenceEnd {
        tier: "reasoning".into(),
        op: "execute".into(),
        model: None,
        duration_ms: 10,
        usage: Some(Usage {
            input_tokens: 800,
            output_tokens: 90,
        }),
        content: None,
        reasoning: None,
        tool_calls: None,
    });
    rt.script_event(TuiEvent::TaskComplete {
        digest: TaskDigest {
            task_id: "t1".into(),
            status: "done".into(),
            digest: "did the thing".into(),
            result_ref: None,
            usage: Some(Usage {
                input_tokens: 5000,
                output_tokens: 700,
            }),
            depth: 2,
        },
    });
}

#[test]
fn usage_tab_folds_tiers_and_tasks() {
    let (mut app, rt) = usage_app();
    script_usage(&rt);
    app.refresh_snapshot();
    let out = render(&mut app, 140, 40);
    assert!(out.contains("This session"), "session header: {out}");
    assert!(out.contains("orchestrator"), "orchestrator row: {out}");
    assert!(out.contains("1200"), "orchestrator input: {out}");
    assert!(out.contains("reasoning"), "reasoning row: {out}");
    assert!(out.contains("sub-agents"), "sub-agent row: {out}");
    assert!(out.contains("5000"), "task input tokens: {out}");
    assert!(out.contains("t1"), "task row: {out}");
}

#[test]
fn usage_tab_without_backend_shows_login_hint() {
    let (mut app, _rt) = usage_app();
    let out = render(&mut app, 140, 40);
    assert!(out.contains("no model calls yet"), "empty session: {out}");
    assert!(out.contains("medulla login"), "login hint: {out}");
}

#[test]
fn account_panel_renders_backend_payload() {
    let (mut app, _rt) = usage_app();
    app.set_account_usage(Some(json!({
        "plan": "pro",
        "inferenceTotals": { "spent": 1.2345, "calls": 42 },
        "remainingUsd": 8.7655,
        "inferenceByModel": [
            { "model": "reasoning-v1", "spent": 1.0 },
            { "model": "summarization-v1", "spent": 0.2345 }
        ]
    })));
    let out = render(&mut app, 140, 40);
    assert!(out.contains("plan       pro"), "plan: {out}");
    assert!(out.contains("$1.2345 spent"), "spent: {out}");
    assert!(out.contains("42 calls"), "calls: {out}");
    assert!(out.contains("remaining  $8.7655"), "remaining: {out}");
    assert!(out.contains("reasoning-v1"), "model row: {out}");
    assert!(out.contains("summarization-v1"), "model row: {out}");
}

#[test]
fn refresh_key_requests_usage_and_c_toggles_config() {
    let (mut app, _rt) = usage_app();
    let cmd = key(&mut app, KeyCode::Char('r'));
    assert!(
        matches!(cmd, Some(medulla_tui::ui::app::Cmd::LoadUsage)),
        "r requests a usage refresh"
    );
    // c jumps to the Config subpage (the effective-config view).
    let _ = key(&mut app, KeyCode::Char('c'));
    assert_eq!(app.settings_subpage(), "Config");
    let out = render(&mut app, 200, 50);
    assert!(
        out.contains("Effective configuration ·"),
        "config view: {out}"
    );
    // Number key 1 jumps back to the Usage subpage.
    let _ = key(&mut app, KeyCode::Char('1'));
    assert_eq!(app.settings_subpage(), "Usage");
    let out = render(&mut app, 200, 50);
    assert!(out.contains("This session"), "back to usage: {out}");
}

#[test]
fn tab_cycle_into_usage_requests_load() {
    let (mut app, _rt) = usage_app();
    // Walk the tab ring one full lap; entering Usage must yield LoadUsage.
    app.tab_index = 0;
    let mut saw_load = false;
    for _ in 0..TABS.len() {
        if let Some(medulla_tui::ui::app::Cmd::LoadUsage) = key(&mut app, KeyCode::Tab) {
            saw_load = true;
        }
    }
    assert!(saw_load, "cycling through Usage requests an account fetch");
}
