//! Unit tests for the Subconscious live signal tab.
//!
//! Pinned through a rendered buffer rather than by calling the draw helper: the
//! point of the tab is what an operator reads on it, and the shortcut line is
//! part of that — a placeholder that advertised the session steering chords
//! would teach keys it does not bind.

use std::sync::Arc;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use crate::ui::app::{App, TABS};

/// Render the whole screen with the Subconscious tab selected.
fn render_subconscious() -> String {
    let rt: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut app = App::new(rt, LoadedConfig::defaults("medulla.tui.json".into()));
    app.tab_index = TABS
        .iter()
        .position(|tab| *tab == "Subconscious")
        .expect("the Subconscious tab is listed");
    let mut terminal = Terminal::new(TestBackend::new(100, 32)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    buffer
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>()
}

#[test]
fn the_live_signal_field_has_a_clear_status_and_animated_graph() {
    let out = render_subconscious();
    assert!(
        out.contains("Quietly processing"),
        "the status panel is missing: {out}"
    );
    assert!(
        out.contains("LIVE OBSERVATION"),
        "the live status should be explicit: {out}"
    );
    assert!(
        out.contains("Signal field · live"),
        "the animated graph has moved here: {out}"
    );
}

#[test]
fn the_shortcut_line_advertises_decision_review_not_session_chords() {
    let out = render_subconscious();
    assert!(
        out.contains("live signal field · E decisions"),
        "the tab needs its own shortcut line"
    );
    assert!(
        !out.contains("close session"),
        "the default session hints must not leak onto a tab that binds nothing"
    );
}
