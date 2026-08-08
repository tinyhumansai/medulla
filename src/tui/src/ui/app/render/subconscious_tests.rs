//! Unit tests for the Subconscious placeholder tab.
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
fn the_placeholder_names_the_three_surfaces_that_will_land_here() {
    let out = render_subconscious();
    assert!(
        out.contains("Subconscious · Coming soon"),
        "the panel title is missing"
    );
    for section in ["Intake", "Learnings", "Approvals"] {
        assert!(out.contains(section), "{section} is missing from the card");
    }
    assert!(
        out.contains("Nothing here is live yet"),
        "the card must say it is not wired yet"
    );
}

#[test]
fn the_shortcut_line_does_not_advertise_the_session_chords() {
    let out = render_subconscious();
    assert!(
        out.contains("Subconscious coming soon"),
        "the tab needs its own shortcut line"
    );
    assert!(
        !out.contains("close session"),
        "the default session hints must not leak onto a tab that binds nothing"
    );
}
