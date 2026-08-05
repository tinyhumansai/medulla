//! Feature tests for the Settings tab: its grouped subpage nav, number-key and
//! arrow navigation, that every subpage renders, the Appearance theme editor
//! (live-applies + persists), and the unified themed selection highlight.
//!
//! Settings also hosts what used to be the Trace and Context tabs,
//! so the tab bar's shrinking is asserted here too.

use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Color;
use ratatui::Terminal;

use medulla::config::{AppearanceConfig, LinkConfig, LoadedConfig, ResourceDisplay};
use medulla::runtime::mock::MockRuntime;
use medulla_tui::ui::app::{App, Cmd, TABS};
use medulla_tui::ui::resources::DeviceSnapshot;

fn loaded() -> LoadedConfig {
    let mut l = LoadedConfig::defaults("medulla.tui.json".into());
    l.config.link = Some(LinkConfig::default());
    l
}

fn settings_app() -> App {
    let rt = Arc::new(MockRuntime::demo());
    let mut app = App::new(rt, loaded());
    app.tab_index = TABS.iter().position(|t| *t == "Settings").unwrap();
    app
}

fn key(app: &mut App, code: KeyCode) -> Option<Cmd> {
    app.on_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)))
}

fn draw(app: &mut App, w: u16, h: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    terminal.backend().buffer().clone()
}

fn text_of(buf: &Buffer) -> String {
    buf.content().iter().map(|c| c.symbol()).collect()
}

fn any_cell_with_bg(buf: &Buffer, bg: Color) -> bool {
    buf.content().iter().any(|c| c.bg == bg)
}

#[test]
fn settings_tab_renders_nav_and_default_usage_subpage() {
    let mut app = settings_app();
    let out = text_of(&draw(&mut app, 140, 40));
    // Left nav lists every subpage.
    for name in [
        "Usage",
        "Appearance",
        "Status line",
        "Config",
        "Feedback",
        "Trace",
        "Context",
        "Account",
        "Help",
    ] {
        assert!(out.contains(name), "nav missing {name}: {out}");
    }
    // Default subpage is Usage.
    assert_eq!(app.settings_subpage(), "Usage");
    assert!(out.contains("This session"), "usage content: {out}");
}

#[test]
fn number_keys_jump_subpages() {
    let mut app = settings_app();
    let _ = key(&mut app, KeyCode::Char('2'));
    assert_eq!(app.settings_subpage(), "Appearance");
    let _ = key(&mut app, KeyCode::Char('3'));
    assert_eq!(app.settings_subpage(), "Status line");
    let _ = key(&mut app, KeyCode::Char('4'));
    assert_eq!(app.settings_subpage(), "Config");
    // Help is the last subpage — ninth, now that Status line is on the nav.
    let _ = key(&mut app, KeyCode::Char('9'));
    assert_eq!(app.settings_subpage(), "Help");
    // Render the full help page: this test verifies numeric subpage navigation,
    // while short-viewport scrolling has its own focused coverage.
    let out = text_of(&draw(&mut app, 140, 64));
    assert!(out.contains("Commands"), "help subpage: {out}");
    // Jumping to Usage requests an account-usage fetch.
    let cmd = key(&mut app, KeyCode::Char('1'));
    assert!(
        matches!(cmd, Some(Cmd::LoadUsage)),
        "usage jump loads usage"
    );
}

#[test]
fn arrow_keys_move_subpage_selector() {
    let mut app = settings_app();
    assert_eq!(app.settings_subpage(), "Usage");
    let _ = key(&mut app, KeyCode::Down);
    assert_eq!(app.settings_subpage(), "Appearance");
    let _ = key(&mut app, KeyCode::Down);
    assert_eq!(app.settings_subpage(), "Status line");
    let _ = key(&mut app, KeyCode::Up);
    assert_eq!(app.settings_subpage(), "Appearance");
}

#[test]
fn status_line_selection_scrolls_into_view_on_a_short_terminal() {
    let mut app = settings_app();
    let _ = key(&mut app, KeyCode::Char('3'));
    // Walk to the final path-style qualifier. Thread name adds two rows ahead
    // of the path group, so this must cover the complete status-line catalog.
    for _ in 0..14 {
        let _ = key(&mut app, KeyCode::Down);
    }

    let out = text_of(&draw(&mut app, 80, 24));

    assert!(
        out.contains("shortened"),
        "the selected path-style value must remain visible: {out}"
    );
}

#[test]
fn appearance_cycling_changes_live_theme() {
    let mut app = settings_app();
    let _ = key(&mut app, KeyCode::Char('2')); // Appearance
    assert_eq!(app.theme_primary(), Color::Red);
    // The primary role is selected first; Right enters the next editor color.
    let _ = key(&mut app, KeyCode::Right);
    assert_eq!(app.theme_primary(), Color::Cyan);
    // A selected row is now highlighted with the new primary background.
    let buf = draw(&mut app, 140, 40);
    assert!(
        any_cell_with_bg(&buf, Color::Cyan),
        "selection uses the live primary as background"
    );
    // Left steps back.
    let _ = key(&mut app, KeyCode::Left);
    assert_eq!(app.theme_primary(), Color::Red);
}

#[test]
fn appearance_jk_selects_role_before_cycling() {
    let mut app = settings_app();
    let _ = key(&mut app, KeyCode::Char('2')); // Appearance
    let primary_before = app.theme_primary();
    // Move off the primary role, then cycle: primary must be untouched.
    let _ = key(&mut app, KeyCode::Char('j')); // select accent
    let _ = key(&mut app, KeyCode::Enter); // cycle accent
    assert_eq!(app.theme_primary(), primary_before, "primary unchanged");
}

#[test]
fn appearance_persists_theme_to_injected_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut app = settings_app();
    app.set_config_path(path.clone());
    let _ = key(&mut app, KeyCode::Char('2')); // Appearance
    let _ = key(&mut app, KeyCode::Right); // cycle primary
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("[theme]"), "theme section written: {text}");
    assert!(text.contains("primary = \"cyan\""), "primary saved: {text}");
    assert!(
        app.status().contains("saved"),
        "status note: {}",
        app.status()
    );
}

#[test]
fn appearance_blink_status_reports_the_boolean_value() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut app = settings_app();
    app.set_config_path(path.clone());
    let _ = key(&mut app, KeyCode::Char('2'));
    for _ in 0..5 {
        let _ = key(&mut app, KeyCode::Char('j'));
    }
    let _ = key(&mut app, KeyCode::Enter);

    assert!(app.status().contains("Attention blink → off (saved)"));
    let saved = std::fs::read_to_string(path).unwrap();
    assert!(saved.contains("attentionBlink = false"), "{saved}");
}

#[test]
fn appearance_cycles_and_persists_process_indicators() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut app = settings_app();
    app.set_config_path(path.clone());
    let _ = key(&mut app, KeyCode::Char('2'));
    // Five color rows and the attention blink toggle precede resources.
    for _ in 0..6 {
        let _ = key(&mut app, KeyCode::Char('j'));
    }
    let _ = key(&mut app, KeyCode::Right);

    let out = text_of(&draw(&mut app, 180, 45));
    assert!(out.contains("Process CPU          percent"), "{out}");
    let saved = std::fs::read_to_string(path).unwrap();
    assert!(saved.contains("[appearance]"), "{saved}");
    assert!(saved.contains("cpu = \"percent\""), "{saved}");
    assert!(saved.contains("diskIo = \"off\""), "{saved}");
}

#[test]
fn appearance_persists_process_indicators_to_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("medulla.tui.json");
    std::fs::write(&path, r#"{"unrelated":{"kept":true}}"#).unwrap();
    let mut app = settings_app();
    app.set_config_path(path.clone());
    let _ = key(&mut app, KeyCode::Char('2'));
    // Five color rows and the attention blink toggle precede resources.
    for _ in 0..6 {
        let _ = key(&mut app, KeyCode::Char('j'));
    }
    let _ = key(&mut app, KeyCode::Right);

    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(saved["appearance"]["cpu"], "percent");
    assert_eq!(saved["appearance"]["diskIo"], "off");
    assert_eq!(saved["unrelated"]["kept"], true);
}

/// A fixed reading, so the sidebar says the same thing on every machine.
fn device_sample() -> DeviceSnapshot {
    DeviceSnapshot {
        cpu_fraction: Some(0.42),
        memory_used_bytes: Some(8 * 1024 * 1024 * 1024),
        memory_total_bytes: Some(32 * 1024 * 1024 * 1024),
        disk_used_bytes: Some(300 * 1024 * 1024 * 1024),
        disk_total_bytes: Some(400 * 1024 * 1024 * 1024),
    }
}

/// Open the Agents tab with the given appearance and an injected device sample.
fn agents_app(appearance: AppearanceConfig) -> App {
    let mut config = loaded();
    config.config.appearance = appearance;
    let runtime = Arc::new(MockRuntime::demo());
    let mut app = App::new(runtime, config);
    app.set_device_snapshot(device_sample());
    app.tab_index = TABS.iter().position(|tab| *tab == "Agents").unwrap();
    app
}

#[test]
fn enabled_device_indicators_render_in_the_agents_sidebar() {
    let mut app = agents_app(AppearanceConfig {
        device_cpu: ResourceDisplay::Percent,
        device_ram: ResourceDisplay::Value,
        device_disk: ResourceDisplay::Percent,
        ..AppearanceConfig::default()
    });
    let out = text_of(&draw(&mut app, 140, 40));
    for label in ["Device CPU 42%", "Device RAM 8G/32G", "Device disk 75%"] {
        assert!(out.contains(label), "missing {label}: {out}");
    }
}

#[test]
fn device_indicators_stay_off_by_default() {
    let mut app = agents_app(AppearanceConfig::default());
    let out = text_of(&draw(&mut app, 140, 40));
    assert!(!out.contains("Device"), "{out}");
}

#[test]
fn a_narrow_sidebar_keeps_navigation_and_drops_device_detail() {
    let mut app = agents_app(AppearanceConfig {
        device_cpu: ResourceDisplay::Bar,
        device_ram: ResourceDisplay::Value,
        device_disk: ResourceDisplay::Value,
        ..AppearanceConfig::default()
    });
    let out = text_of(&draw(&mut app, 56, 20));
    // A narrow rail fits a bar but not a `used/total` pair, so the byte counts
    // collapse to percentages rather than spilling past the border.
    assert!(out.contains("Device CPU"), "{out}");
    assert!(out.contains("Device RAM 25%"), "{out}");
    // Navigation survives: the lane rows are still on screen above the footer.
    assert!(out.contains("orchestrator"), "{out}");
}

#[test]
fn appearance_cycles_and_persists_device_indicators_independently() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut app = settings_app();
    app.set_config_path(path.clone());
    let _ = key(&mut app, KeyCode::Char('2'));
    // Five color rows, attention blink, three process indicators, and Session titles
    // lands on Device CPU.
    for _ in 0..10 {
        let _ = key(&mut app, KeyCode::Char('j'));
    }
    let _ = key(&mut app, KeyCode::Right);

    let out = text_of(&draw(&mut app, 180, 45));
    assert!(out.contains("Device CPU           percent"), "{out}");
    let saved = std::fs::read_to_string(path).unwrap();
    assert!(saved.contains("deviceCpu = \"percent\""), "{saved}");
    // The process indicators are untouched by a device row moving.
    assert!(saved.contains("cpu = \"off\""), "{saved}");
    assert!(saved.contains("deviceRam = \"off\""), "{saved}");
}

#[test]
fn enabled_process_indicators_render_on_the_status_line() {
    let mut config = loaded();
    config.config.appearance = AppearanceConfig {
        cpu: ResourceDisplay::Bar,
        ram: ResourceDisplay::Bar,
        disk_io: ResourceDisplay::Bar,
        ..AppearanceConfig::default()
    };
    let runtime = Arc::new(MockRuntime::demo());
    let mut app = App::new(runtime, config);
    let out = text_of(&draw(&mut app, 80, 40));
    for label in ["CPU", "RAM", "IO"] {
        assert!(out.contains(label), "missing {label}: {out}");
    }
}

#[test]
fn selection_rows_use_theme_primary_background() {
    // The Settings nav's selected subpage row is highlighted with primary red.
    let mut app = settings_app();
    let buf = draw(&mut app, 140, 40);
    assert!(
        any_cell_with_bg(&buf, Color::Red),
        "selected nav row uses primary background"
    );
}

#[test]
fn each_settings_subpage_renders_its_signature() {
    // Trace and Context moved under Settings > DEBUG.
    let signatures = [
        ("Usage", "This session"),
        ("Appearance", "Appearance"),
        ("Status line", "Preview"),
        ("Config", "Effective configuration ·"),
        ("Trace", "Trace ·"),
        ("Context", "Environment ·"),
        ("Account", "Account"),
        ("Help", "Keyboard & REPL help"),
    ];
    for (name, sig) in signatures {
        let mut app = settings_app();
        let _ = app.focus_settings_subpage(name);
        let out = text_of(&draw(&mut app, 160, 50));
        assert!(out.contains("Tab views"), "{name}: missing shortcut line");
        assert!(
            out.contains(sig),
            "{name}: missing signature {sig:?}: {out}"
        );
    }
}

#[test]
fn config_subpage_shows_effective_router_without_the_key_value() {
    // Step 4: the Config subpage surfaces the effective router — the endpoint each
    // harness routes to and the key env-var NAME with its set/missing state — so an
    // operator can confirm routing without reading harness configs. The key VALUE
    // must never render.
    const KEY_ENV: &str = "MEDULLA_FEATURE_ROUTER_KEY";
    const SECRET: &str = "sk-feature-secret-must-not-render-7c1";
    std::env::set_var(KEY_ENV, SECRET);

    let mut l = loaded();
    let mut providers = std::collections::HashMap::new();
    providers.insert(
        "claude".to_string(),
        medulla::config::RouterProviderConfig {
            base_url: Some("https://gw.example/anthropic".into()),
        },
    );
    l.config.router = Some(medulla::config::RouterConfig {
        base_url: Some("https://gw.example/v1".into()),
        api_key_env: Some(KEY_ENV.into()),
        models: std::collections::HashMap::new(),
        providers,
    });

    let rt = Arc::new(MockRuntime::demo());
    let mut app = App::new(rt, l);
    app.tab_index = TABS.iter().position(|t| *t == "Settings").unwrap();
    let _ = app.focus_settings_subpage("Config");
    let out = text_of(&draw(&mut app, 160, 60));

    assert!(out.contains("Router (effective)"), "router block: {out}");
    // codex/opencode inherit the top-level endpoint; claude uses its override.
    assert!(
        out.contains("https://gw.example/v1"),
        "top-level endpoint shown: {out}"
    );
    assert!(
        out.contains("https://gw.example/anthropic"),
        "claude override shown: {out}"
    );
    // The key is referenced by NAME and marked set — never the value.
    assert!(out.contains(KEY_ENV), "key env var name shown: {out}");
    assert!(out.contains("(set)"), "key presence shown: {out}");
    assert!(
        !out.contains(SECRET),
        "the key VALUE must never render in the diagnostic"
    );

    std::env::remove_var(KEY_ENV);
}

#[test]
fn the_settings_nav_groups_its_subpages() {
    let mut app = settings_app();
    let _ = app.focus_settings_subpage("Usage");
    let out = text_of(&draw(&mut app, 160, 50));
    for heading in ["GENERAL", "DEBUG", "ABOUT"] {
        assert!(
            out.contains(heading),
            "missing nav heading {heading}: {out}"
        );
    }
}

#[test]
fn secondary_and_paused_surfaces_are_not_top_level_tabs() {
    for gone in ["Trace", "Context", "TokenMaxxxing"] {
        assert!(
            !TABS.contains(&gone),
            "{gone} should not appear in the tab bar"
        );
    }
}

#[test]
fn tab_leaves_the_settings_tab_from_both_focus_states() {
    // Regression: the subpage nav used to swallow every key it did not bind,
    // including Tab. Since the nav is where you land on entering Settings, that
    // trapped the keyboard in the tab with no way out.
    let mut app = settings_app();
    assert!(
        !app.settings_focused(),
        "entering Settings lands on the nav"
    );
    let _ = key(&mut app, KeyCode::Tab);
    assert_ne!(app.tab(), "Settings", "Tab escapes from the nav");

    // And from inside a focused content pane.
    let mut app = settings_app();
    let _ = key(&mut app, KeyCode::Enter);
    assert!(app.settings_focused());
    let _ = key(&mut app, KeyCode::Tab);
    assert_ne!(app.tab(), "Settings", "Tab escapes from a focused page");

    // BackTab too, since it is the only way back to the previous tab.
    let mut app = settings_app();
    let _ = key(&mut app, KeyCode::BackTab);
    assert_ne!(app.tab(), "Settings", "BackTab escapes from the nav");
}
