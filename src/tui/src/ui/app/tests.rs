//! Focused unit tests for the [`App`] screen: that every tab renders, the async
//! header toggle shows, and the composer/slash-command dispatch behaves.

use super::*;
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn app() -> App {
    let rt: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let loaded = {
        let mut l = LoadedConfig::defaults("medulla.tui.json".into());
        l.config.tinyplace = Some(medulla::config::TinyplaceConfig::default());
        l
    };
    App::new(rt, loaded)
}

fn render(app: &mut App) -> String {
    let backend = TestBackend::new(100, 32);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    buf.content().iter().map(|c| c.symbol()).collect::<String>()
}

#[test]
fn every_tab_renders() {
    for (i, name) in TABS.iter().enumerate() {
        let mut a = app();
        a.tab_index = i;
        let out = render(&mut a);
        assert!(out.contains("MEDULLA"), "tab {name} missing header");
    }
}

#[test]
fn header_shows_async_toggle() {
    let mut a = app();
    a.runtime.set_async_mode(true);
    a.refresh_snapshot();
    let out = render(&mut a);
    assert!(out.contains("ASYNC ON"));
}

#[test]
fn slash_help_switches_tab() {
    let mut a = app();
    a.tab_index = 1;
    let _ = a.execute("/help".into());
    assert_eq!(a.tab(), "Settings");
    assert_eq!(a.settings_subpage(), "Help");
}

#[test]
fn unknown_command_sets_status() {
    let mut a = app();
    let _ = a.execute("/bogus".into());
    assert!(a.status.contains("Unknown command"));
}

#[test]
fn plain_text_returns_submit_cmd() {
    let mut a = app();
    a.tab_index = 1;
    let cmd = a.execute("hello world".into());
    assert!(matches!(cmd, Some(Cmd::Submit(s)) if s == "hello world"));
    assert_eq!(a.status, "Cycle running…");
}

#[test]
fn typing_inserts_into_draft() {
    let mut a = app();
    a.tab_index = 1;
    for ch in "hi".chars() {
        a.on_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    assert_eq!(a.draft.text, "hi");
    a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    assert_eq!(a.draft.text, "hi\n");
}
