//! Feature tests for the Settings tab: its subpage nav (Usage / Appearance /
//! Config / Help), number-key and arrow navigation, the Appearance theme editor
//! (live-applies + persists), and the unified themed selection highlight.

use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Color;
use ratatui::Terminal;

use medulla::config::{LoadedConfig, TinyplaceConfig};
use medulla::runtime::mock::MockRuntime;
use medulla_tui::ui::app::{App, Cmd, TABS};

fn loaded() -> LoadedConfig {
    let mut l = LoadedConfig::defaults("medulla.tui.json".into());
    l.config.tinyplace = Some(TinyplaceConfig::default());
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
    for name in ["Usage", "Appearance", "Config", "Help"] {
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
    assert_eq!(app.settings_subpage(), "Config");
    let _ = key(&mut app, KeyCode::Char('4'));
    assert_eq!(app.settings_subpage(), "Help");
    let out = text_of(&draw(&mut app, 140, 40));
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
    assert_eq!(app.settings_subpage(), "Config");
    let _ = key(&mut app, KeyCode::Up);
    assert_eq!(app.settings_subpage(), "Appearance");
}

#[test]
fn appearance_cycling_changes_live_theme() {
    let mut app = settings_app();
    let _ = key(&mut app, KeyCode::Char('2')); // Appearance
    assert_eq!(app.theme_primary(), Color::Cyan);
    // The primary role is selected first; Right cycles it to the next palette color.
    let _ = key(&mut app, KeyCode::Right);
    assert_eq!(app.theme_primary(), Color::LightCyan);
    // A selected row is now highlighted with the new primary background.
    let buf = draw(&mut app, 140, 40);
    assert!(
        any_cell_with_bg(&buf, Color::LightCyan),
        "selection uses the live primary as background"
    );
    // Left steps back.
    let _ = key(&mut app, KeyCode::Left);
    assert_eq!(app.theme_primary(), Color::Cyan);
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
    assert!(
        text.contains("primary = \"lightcyan\""),
        "primary saved: {text}"
    );
    assert!(
        app.status().contains("saved"),
        "status note: {}",
        app.status()
    );
}

#[test]
fn selection_rows_use_theme_primary_background() {
    // The Settings nav's selected subpage row is highlighted with primary (Cyan).
    let mut app = settings_app();
    let buf = draw(&mut app, 140, 40);
    assert!(
        any_cell_with_bg(&buf, Color::Cyan),
        "selected nav row uses primary background"
    );
}
