//! Shared setup for the `feature_paste` test binary: apps parked on the tab
//! under test, synthetic crossterm event builders, and the `paste` driver every
//! group uses.
//!
//! Re-exports the types the grouped modules need so they can
//! `use crate::helpers::*;`.

pub use std::sync::Arc;

pub use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

pub use medulla::config::{LinkConfig, LoadedConfig};
pub use medulla::runtime::mock::MockRuntime;
pub use medulla_tui::ui::app::{App, Cmd, TABS};

pub fn loaded() -> LoadedConfig {
    let mut l = LoadedConfig::defaults("medulla.tui.json".into());
    l.config.link = Some(LinkConfig::default());
    l
}

/// An app on the Agents tab, where the chat composer lives.
pub fn agents_app() -> App {
    let mut app = App::new(Arc::new(MockRuntime::empty()), loaded());
    app.tab_index = tab_index("Sessions");
    app
}

/// An Agents-tab app with lanes on its rail, so the cursor can be moved off the
/// orchestrator onto a row that draws no composer.
pub fn demo_agents_app() -> App {
    let mut app = App::new(Arc::new(MockRuntime::demo()), loaded());
    app.tab_index = tab_index("Sessions");
    app.refresh_snapshot();
    app
}

pub fn tab_index(name: &str) -> usize {
    TABS.iter().position(|t| *t == name).unwrap_or(0)
}

pub fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

pub fn key_press(app: &mut App, code: KeyCode) {
    let _ = app.on_event(key(code));
}

pub fn paste(app: &mut App, text: &str) -> Option<Cmd> {
    app.on_event(Event::Paste(text.into()))
}
