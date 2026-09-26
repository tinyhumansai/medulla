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
        "Subscriptions",
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
    assert_eq!(app.settings_subpage(), "Subscriptions");
    let _ = key(&mut app, KeyCode::Char('4'));
    assert_eq!(app.settings_subpage(), "Status line");
    let _ = key(&mut app, KeyCode::Char('5'));
    assert_eq!(app.settings_subpage(), "Config");
    // Help is the last subpage, and past the nine a digit can reach now that
    // Subscriptions is on the nav — so the arrows are what get there.
    let _ = key(&mut app, KeyCode::Char('9'));
    assert_eq!(app.settings_subpage(), "Account");
    // A digit also enters the pane, so getting to Help means stepping back out
    // to the nav first — the arrows browse content once a page has focus.
    let _ = key(&mut app, KeyCode::Esc);
    let _ = key(&mut app, KeyCode::Down);
    assert_eq!(app.settings_subpage(), "Help");
    // Render the full help page: this test verifies numeric subpage navigation,
    // while short-viewport scrolling has its own focused coverage.
    let out = text_of(&draw(&mut app, 140, 64));
    assert!(out.contains("Keyboard & REPL help"), "help subpage: {out}");
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
    assert_eq!(app.settings_subpage(), "Subscriptions");
    let _ = key(&mut app, KeyCode::Up);
    assert_eq!(app.settings_subpage(), "Appearance");
}

#[test]
fn status_line_selection_scrolls_into_view_on_a_short_terminal() {
    let mut app = settings_app();
    let _ = key(&mut app, KeyCode::Char('4'));
    // Walk to the final path-style qualifier. Thread name and worktree each add
    // two rows ahead of the path group, so this must cover the complete
    // status-line catalog.
    for _ in 0..16 {
        let _ = key(&mut app, KeyCode::Down);
    }

    let out = text_of(&draw(&mut app, 80, 24));

    assert!(
        out.contains("shortened"),
        "the selected path-style value must remain visible: {out}"
    );
}

#[test]
fn a_status_line_field_is_headed_described_and_its_choices_spelled_out() {
    let mut app = settings_app();
    let _ = key(&mut app, KeyCode::Char('4'));
    // Down four times: the harness group's "spelled" row.
    for _ in 0..4 {
        let _ = key(&mut app, KeyCode::Down);
    }

    let out = text_of(&draw(&mut app, 100, 40));

    assert!(
        out.contains("Harness name") && out.contains("which CLI is driving the session"),
        "the field is headed and described where its rows are: {out}"
    );
    for choice in ["long", "short", "icon"] {
        assert!(
            out.contains(choice),
            "the footer lists every value ←/→ can reach, not only the current one; \
             missing {choice}: {out}"
        );
    }
    assert!(
        out.contains("statusLine.harnessStyle"),
        "the footer names the key the answer is written to: {out}"
    );
    assert!(
        out.contains("Claude Code, claude, or just the provider icon."),
        "the footer explains what the selected row does: {out}"
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

/// The pulse rate is configured, reported, and persisted in seconds — the unit
/// the effect is actually judged in.
#[test]
fn appearance_cycles_the_attention_blink_rate_in_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut app = settings_app();
    app.set_config_path(path.clone());
    let _ = key(&mut app, KeyCode::Char('2'));
    // Five colour rows and the blink toggle land on the rate.
    for _ in 0..6 {
        let _ = key(&mut app, KeyCode::Char('j'));
    }
    let _ = key(&mut app, KeyCode::Right);

    assert!(
        app.status().contains("Attention blink rate → 1.5s (saved)"),
        "status note: {}",
        app.status()
    );
    let out = text_of(&draw(&mut app, 180, 45));
    assert!(out.contains("Blink rate           1.5s"), "{out}");
    let saved = std::fs::read_to_string(path).unwrap();
    assert!(saved.contains("attentionBlinkSeconds = 1.5"), "{saved}");
}

/// Custom rates enter the offered cycle from the direction the operator chose.
#[test]
fn appearance_cycles_custom_attention_blink_rates_from_each_end() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut app = settings_app();
    app.set_config_path(path);
    app.set_attention_blink_ms(1_100);
    let _ = key(&mut app, KeyCode::Char('2'));
    for _ in 0..6 {
        let _ = key(&mut app, KeyCode::Char('j'));
    }

    let _ = key(&mut app, KeyCode::Right);
    assert!(app.status().contains("Attention blink rate → 0.3s (saved)"));

    app.set_attention_blink_ms(1_100);
    let _ = key(&mut app, KeyCode::Left);
    assert!(app.status().contains("Attention blink rate → 3.0s (saved)"));

    let _ = key(&mut app, KeyCode::Left);
    assert!(app.status().contains("Attention blink rate → 2.0s (saved)"));
}

#[test]
fn appearance_cycles_and_persists_process_indicators() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut app = settings_app();
    app.set_config_path(path.clone());
    let _ = key(&mut app, KeyCode::Char('2'));
    // Five color rows and the two attention controls precede resources.
    for _ in 0..7 {
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
fn appearance_cycles_and_persists_the_sidebar_layout() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut app = settings_app();
    app.set_config_path(path.clone());
    let _ = key(&mut app, KeyCode::Char('2'));
    // Five color rows, the two attention controls, and the seven option rows
    // before it.
    for _ in 0..14 {
        let _ = key(&mut app, KeyCode::Char('j'));
    }
    let _ = key(&mut app, KeyCode::Right);

    let out = text_of(&draw(&mut app, 180, 60));
    assert!(out.contains("Group by             path"), "{out}");
    assert!(
        app.status().contains("Sidebar grouping \u{2192} path"),
        "status names the setting and its new value: {}",
        app.status()
    );
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("sidebarGrouping = \"path\""), "{saved}");

    // The sort row is the next one down, and cycles independently.
    let _ = key(&mut app, KeyCode::Char('j'));
    let _ = key(&mut app, KeyCode::Right);

    let out = text_of(&draw(&mut app, 180, 60));
    assert!(out.contains("Sort by              recent"), "{out}");
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("sidebarSort = \"recent\""), "{saved}");
    assert!(
        saved.contains("sidebarGrouping = \"path\""),
        "the grouping survives the next write: {saved}"
    );
}

#[test]
fn appearance_sidebar_grouping_wraps_backwards() {
    // Left from the default is the last value, not a stuck row: the cycle is
    // how every other control on this page behaves.
    let mut app = settings_app();
    let _ = key(&mut app, KeyCode::Char('2'));
    for _ in 0..14 {
        let _ = key(&mut app, KeyCode::Char('j'));
    }
    let _ = key(&mut app, KeyCode::Left);

    let out = text_of(&draw(&mut app, 180, 60));
    assert!(out.contains("Group by             none"), "{out}");
}

#[test]
fn appearance_persists_process_indicators_to_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("medulla.tui.json");
    std::fs::write(&path, r#"{"unrelated":{"kept":true}}"#).unwrap();
    let mut app = settings_app();
    app.set_config_path(path.clone());
    let _ = key(&mut app, KeyCode::Char('2'));
    // Five color rows and the two attention controls precede resources.
    for _ in 0..7 {
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
    app.tab_index = TABS.iter().position(|tab| *tab == "Sessions").unwrap();
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
    // Each reading is labelled on the left and flushed to the rail's right
    // edge, so the value is what sits against the border.
    for (label, value) in [
        ("Device CPU", " 42%│"),
        ("Device RAM", "8G/32G│"),
        ("Device disk", " 75%│"),
    ] {
        assert!(out.contains(label), "missing {label}: {out}");
        assert!(out.contains(value), "{label} is not flush right: {out}");
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
    assert!(out.contains("Device RAM"), "{out}");
    assert!(
        out.contains(" 25%│"),
        "the percentage is flush right: {out}"
    );
    // Navigation survives: the rail's own rows are still on screen above the
    // footer rather than being crowded out by it.
    assert!(out.contains("task-1"), "{out}");
}

#[test]
fn appearance_cycles_and_persists_device_indicators_independently() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut app = settings_app();
    app.set_config_path(path.clone());
    let _ = key(&mut app, KeyCode::Char('2'));
    // Five color rows, the two attention controls, three process indicators,
    // and Session titles lands on Device CPU.
    for _ in 0..11 {
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
        ("Status line", "as on the rail"),
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
        provider_only: Vec::new(),
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

/// The page opens on Options: the tab strip, the grouped rows, and a footer
/// that explains the row under the cursor rather than every group at once.
#[test]
fn appearance_opens_on_options_with_a_footer_for_the_selected_row() {
    let mut app = settings_app();
    let _ = key(&mut app, KeyCode::Char('2'));
    let out = text_of(&draw(&mut app, 150, 45));

    for group in ["Colors", "Attention", "Status line", "Sessions sidebar"] {
        assert!(out.contains(group), "group {group} is headed: {out}");
    }
    assert!(
        out.contains("The colour behind a selected row"),
        "the footer explains the selected row: {out}"
    );
    assert!(
        out.contains("red \u{b7} cyan"),
        "the footer lists every value the row can take: {out}"
    );
}

/// The sidebar's own settings read as one group, under the name the panel
/// actually carries. They used to be split across a "Sessions sidebar" group
/// and an "Agents sidebar" one naming a panel titled "Sessions".
#[test]
fn appearance_keeps_the_sidebar_settings_in_one_group() {
    let mut app = settings_app();
    let _ = key(&mut app, KeyCode::Char('2'));
    let out = text_of(&draw(&mut app, 150, 45));

    assert!(!out.contains("Agents sidebar"), "one sidebar group: {out}");
    let sidebar = out.find("Sessions sidebar").expect("sidebar group");
    for row in ["Session titles", "Device CPU", "Group by", "Sort by"] {
        let at = out
            .find(row)
            .unwrap_or_else(|| panic!("{row} is listed: {out}"));
        assert!(at > sidebar, "{row} sits under the sidebar heading: {out}");
    }
}

/// A wide pane shows the switches beside the preview, with no tab strip and
/// nothing for `Tab` to do.
#[test]
fn appearance_puts_the_switches_beside_the_preview_when_it_fits() {
    let mut app = settings_app();
    let _ = key(&mut app, KeyCode::Char('2'));
    let out = text_of(&draw(&mut app, 150, 45));

    assert!(out.contains("Blink rate"), "the switches are drawn: {out}");
    assert!(
        out.contains("selected row") && out.contains("attention, pulsing"),
        "and so is the preview, captioned: {out}"
    );
    assert!(!out.contains("Tab switches"), "no tab strip: {out}");

    // Tab belongs to the global cycler again while both halves are on screen.
    let _ = key(&mut app, KeyCode::Tab);
    assert_ne!(app.tab(), "Settings", "Tab still cycles the top-level tabs");
}

/// A pane too narrow to halve folds back to two tabs, walked with `Tab`.
#[test]
fn appearance_folds_to_tabs_on_a_narrow_pane() {
    let mut app = settings_app();
    let _ = key(&mut app, KeyCode::Char('2'));
    let out = text_of(&draw(&mut app, 90, 45));

    assert!(
        out.contains("Tab switches"),
        "the tab strip is drawn: {out}"
    );
    assert!(
        out.contains("Blink rate"),
        "Switches is the first tab: {out}"
    );
    assert!(
        !out.contains("attention, pulsing"),
        "the preview is behind the other tab: {out}"
    );

    let _ = key(&mut app, KeyCode::Tab);
    let out = text_of(&draw(&mut app, 90, 45));
    assert_eq!(app.tab(), "Settings", "Tab is the page's own binding here");
    assert!(out.contains("attention, pulsing"), "Tab reveals it: {out}");
    assert!(!out.contains("Blink rate"), "and hides the switches: {out}");
}

/// The preview is drawn from the live config, so a change on the Options tab is
/// visible on the Preview tab — which is the only reason the tab exists.
#[test]
fn appearance_preview_follows_the_sidebar_grouping() {
    let mut app = settings_app();
    let _ = key(&mut app, KeyCode::Char('2'));
    assert!(
        text_of(&draw(&mut app, 150, 45)).contains("workstation"),
        "the default grouping sections the sample by host"
    );

    // Down to Group by, and on to `path`. The preview is beside the row.
    for _ in 0..14 {
        let _ = key(&mut app, KeyCode::Char('j'));
    }
    let _ = key(&mut app, KeyCode::Right);

    let out = text_of(&draw(&mut app, 150, 45));
    assert!(
        out.contains("~/work/medulla"),
        "sectioned by path now: {out}"
    );
    assert!(!out.contains("workstation"), "and no longer by host: {out}");
}

/// An indicator that is switched off is absent from the preview, and the
/// preview says so rather than showing an empty status line.
#[test]
fn appearance_preview_shows_the_indicators_that_are_switched_on() {
    let mut app = settings_app();
    let _ = key(&mut app, KeyCode::Char('2'));
    assert!(
        text_of(&draw(&mut app, 150, 45)).contains("no process indicators are switched on"),
        "the default config enables none of them"
    );

    // Down to Process CPU, and on to `percent`.
    for _ in 0..7 {
        let _ = key(&mut app, KeyCode::Char('j'));
    }
    let _ = key(&mut app, KeyCode::Right);

    let out = text_of(&draw(&mut app, 150, 45));
    assert!(
        out.contains("CPU 25%"),
        "the sample reading is formatted: {out}"
    );
}

/// A wide pane shows every field beside the preview at once. The preview used
/// to sit above the list, and it is fourteen rows tall.
#[test]
fn status_line_puts_every_field_beside_the_preview() {
    let mut app = settings_app();
    let _ = key(&mut app, KeyCode::Char('4'));
    let out = text_of(&draw(&mut app, 140, 40));

    assert!(
        out.contains("as on the rail"),
        "the preview is drawn: {out}"
    );
    assert!(!out.contains("Tab switches"), "no tab strip: {out}");
    for field in [
        "State glyph",
        "Harness name",
        "Managed / unmanaged",
        "Thread name",
        "Git branch",
        "Worktree",
        "Working path",
    ] {
        assert!(out.contains(field), "{field} is on screen at once: {out}");
    }

    // Tab belongs to the global cycler again while both halves are on screen.
    let _ = key(&mut app, KeyCode::Tab);
    assert_ne!(app.tab(), "Settings", "Tab still cycles the top-level tabs");
}

/// The field descriptions moved off the list and into the footer, where only
/// the selected row's is paid for.
#[test]
fn status_line_describes_the_selected_field_in_the_footer() {
    let mut app = settings_app();
    let _ = key(&mut app, KeyCode::Char('4'));
    let out = text_of(&draw(&mut app, 140, 40));

    assert!(
        out.contains("State glyph · the ●/✓/✕ dot"),
        "the footer names the field and what it shows: {out}"
    );
    assert!(
        !out.contains("which CLI is driving the session"),
        "an unselected field's description is not drawn: {out}"
    );

    // Walk into the harness group; the footer follows the cursor.
    for _ in 0..2 {
        let _ = key(&mut app, KeyCode::Char('j'));
    }
    let out = text_of(&draw(&mut app, 140, 40));
    assert!(
        out.contains("Harness name · which CLI is driving the session"),
        "the footer follows the selection: {out}"
    );
}

/// A pane too narrow to halve folds back to two tabs, walked with `Tab`.
#[test]
fn status_line_folds_to_tabs_on_a_narrow_pane() {
    let mut app = settings_app();
    let _ = key(&mut app, KeyCode::Char('4'));
    let out = text_of(&draw(&mut app, 90, 40));

    assert!(
        out.contains("Tab switches"),
        "the tab strip is drawn: {out}"
    );
    assert!(
        out.contains("Thread name"),
        "Switches is the first tab: {out}"
    );
    assert!(
        !out.contains("as on the rail"),
        "the preview is behind the other tab: {out}"
    );

    let _ = key(&mut app, KeyCode::Tab);
    let out = text_of(&draw(&mut app, 90, 40));
    assert_eq!(app.tab(), "Settings", "Tab is the page's own binding here");
    assert!(
        out.contains("as on the rail"),
        "Tab reveals the preview: {out}"
    );
    assert!(
        out.contains("selected") && out.contains("on alert"),
        "each sample is captioned with the condition it stands for: {out}"
    );
    assert!(!out.contains("Thread name"), "and hides the fields: {out}");
}

/// The preview is drawn at a fixed moment against the samples' own timestamps.
/// A wall clock here read `started_at: 0` as an age of fifty-odd years and
/// printed it into the alert row.
#[test]
fn status_line_preview_reports_a_plausible_age_on_the_alert_row() {
    let mut app = settings_app();
    let _ = key(&mut app, KeyCode::Char('4'));

    let out = text_of(&draw(&mut app, 140, 40));
    assert!(out.contains("45s"), "the sample's age is fixed: {out}");
}

/// Changing a field redraws the sample beside it — the reason for the split.
#[test]
fn status_line_preview_redraws_as_the_selected_field_changes() {
    let mut app = settings_app();
    let _ = key(&mut app, KeyCode::Char('4'));
    let before = text_of(&draw(&mut app, 140, 40));

    // The state glyph's position is the selected row; hide it.
    for _ in 0..3 {
        let _ = key(&mut app, KeyCode::Right);
    }
    let after = text_of(&draw(&mut app, 140, 40));

    assert_ne!(before, after, "the sample rows follow the change");
    assert!(
        app.status().contains("state glyph"),
        "the status line names the field: {}",
        app.status()
    );
}
