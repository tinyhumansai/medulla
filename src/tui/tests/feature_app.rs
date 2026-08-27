//! Feature-level tests: drive the `App` like a user via synthetic crossterm
//! events and assert on observable state and the rendered `TestBackend` buffer.
//! These complement the crate's inline unit tests — here we exercise whole flows
//! (typing + submit, slash commands, tab nav, scrolling, working indicator,
//! resume picker, threads/fork, abort/new-session, copy, config rendering).

use std::sync::Arc;

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::config::{LinkConfig, LoadedConfig};
use medulla::runtime::mock::MockRuntime;
use medulla_tui::ui::app::{App, TABS};

// --- harness helpers --------------------------------------------------------

fn loaded() -> LoadedConfig {
    let mut l = LoadedConfig::defaults("medulla.tui.json".into());
    l.config.link = Some(LinkConfig::default());
    l
}

/// App over a populated demo runtime; returns the app and the concrete handle so
/// tests can script events / read recorded calls.
fn demo_app() -> (App, Arc<MockRuntime>) {
    let rt = Arc::new(MockRuntime::demo());
    let app = App::new(rt.clone(), loaded());
    (app, rt)
}

/// App over a bare runtime (no roster, empty chat).
fn empty_app() -> (App, Arc<MockRuntime>) {
    let rt = Arc::new(MockRuntime::empty());
    let app = App::new(rt.clone(), loaded());
    (app, rt)
}

fn key_mod(code: KeyCode, mods: KeyModifiers) -> Event {
    Event::Key(KeyEvent::new(code, mods))
}

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> Event {
    Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

/// Draw once into a fresh backend and flatten the buffer into a string.
fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    let buf = terminal.backend().buffer().clone();
    buf.content().iter().map(|c| c.symbol()).collect()
}

#[test]
fn update_notice_renders_in_header_once_set() {
    let (mut app, _rt) = empty_app();
    // Not shown until the background checker sets it.
    assert!(app.update_notice().is_none());
    let before = render(&mut app, 120, 20);
    assert!(!before.contains("update v9.9.9"));

    app.set_update_notice("update v9.9.9 available — run `medulla update`");
    assert_eq!(
        app.update_notice(),
        Some("update v9.9.9 available — run `medulla update`")
    );
    let after = render(&mut app, 120, 20);
    assert!(
        after.contains("update v9.9.9 available"),
        "header should show the update banner"
    );
}

// --- 1. tab navigation ------------------------------------------------------

#[test]
fn tab_and_backtab_cycle_tabs() {
    let (mut app, _rt) = demo_app();
    assert_eq!(app.tab(), "Sessions");
    let _ = app.on_event(key(KeyCode::Tab));
    assert_eq!(app.tab(), "Workflows");
    // Workflows is followed by the live Subconscious surface.
    let _ = app.on_event(key(KeyCode::Tab));
    assert_eq!(app.tab(), "Subconscious");
    let _ = app.on_event(key_mod(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(app.tab(), "Workflows");
    // Wrap backwards from Sessions to the last tab (Settings).
    let _ = app.on_event(key_mod(KeyCode::BackTab, KeyModifiers::SHIFT));
    let _ = app.on_event(key_mod(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(app.tab(), "Settings");
}

#[test]
fn clicking_tab_bar_selects_tab() {
    let (mut app, _rt) = demo_app();
    // Draw first so the tab hit-boxes are recorded, then click within "Sessions".
    let _ = render(&mut app, 120, 40);
    let _ = app.on_event(mouse(MouseEventKind::Down(MouseButton::Left), 12, 1));
    assert_eq!(app.tab(), "Sessions");
}

#[test]
fn each_tab_renders_its_signature() {
    let signatures = [
        ("Sessions", "Sessions ·"),
        ("Workflows", "Workflows"),
        ("Subconscious", "Signal field · live"),
        ("Hosts", "Hosts"),
        ("Settings", "Settings"),
    ];
    for (name, sig) in signatures {
        let (mut app, _rt) = demo_app();
        app.tab_index = TABS.iter().position(|t| *t == name).unwrap();
        let out = render(&mut app, 120, 40);
        assert!(out.contains("Tab views"), "{name}: missing shortcut line");
        assert!(out.contains(sig), "{name}: missing signature {sig:?}");
    }
}

// --- 2. transcript scroll behavior ------------------------------------------

/// The demo app parked on the dispatched session's own transcript, which is the
/// only scrollable transcript the tab still draws.
fn app_on_a_task_transcript() -> App {
    let (mut app, _rt) = demo_app();
    app.tab_index = TABS
        .iter()
        .position(|tab| *tab == "Sessions")
        .expect("Sessions tab is listed");
    app.refresh_snapshot();
    app
}

#[test]
fn page_up_scrolls_and_page_down_returns_to_bottom() {
    let mut app = app_on_a_task_transcript();
    // Prime area geometry. Narrow on purpose: the transcript has to be taller
    // than the pane for there to be anything below the fold.
    let _ = render(&mut app, 80, 12);
    assert_eq!(app.transcript_scroll(), 0);

    let _ = app.on_event(key(KeyCode::PageUp));
    let _ = render(&mut app, 80, 12);
    assert!(
        app.transcript_scroll() > 0,
        "PageUp should grow the scroll offset"
    );

    let _ = app.on_event(key(KeyCode::PageDown));
    let _ = render(&mut app, 80, 12);
    assert_eq!(
        app.transcript_scroll(),
        0,
        "PageDown should return to the bottom"
    );
}

#[test]
fn wheel_scroll_adjusts_offset_by_three() {
    let mut app = app_on_a_task_transcript();
    let _ = render(&mut app, 80, 12);

    // Over the transcript, not the rail: the wheel acts on the pane under the
    // pointer, and column 10 is the rail on an 80-column screen.
    let _ = app.on_event(mouse(MouseEventKind::ScrollUp, 60, 8));
    assert_eq!(app.transcript_scroll(), 3);
    let _ = app.on_event(mouse(MouseEventKind::ScrollDown, 60, 8));
    assert_eq!(app.transcript_scroll(), 0);
}

#[test]
fn a_wheel_over_the_rail_walks_the_rail_not_the_transcript() {
    let mut app = app_on_a_task_transcript();
    let _ = render(&mut app, 80, 12);

    let _ = app.on_event(mouse(MouseEventKind::ScrollDown, 5, 6));
    assert_eq!(
        app.transcript_scroll(),
        0,
        "the transcript stays where it was"
    );
}

// --- 3. config rendering ----------------------------------------------------

#[test]
fn config_tab_annotates_token_env_presence() {
    // Present env → "(set)". The config view lives behind /config on Usage.
    let set_var = "MEDULLA_FEATURE_TEST_TOKEN_SET";
    std::env::set_var(set_var, "x");
    let (mut app, _rt) = empty_app();
    app.loaded.config.backend.token_env = set_var.into();
    let _ = app.focus_settings_subpage("Config");
    let out = render(&mut app, 200, 50);
    assert!(out.contains("(set)"), "expected (set) annotation");
    std::env::remove_var(set_var);

    // Absent env → "(missing)".
    let missing_var = "MEDULLA_FEATURE_TEST_TOKEN_MISSING";
    std::env::remove_var(missing_var);
    let (mut app, _rt) = empty_app();
    app.loaded.config.backend.token_env = missing_var.into();
    let _ = app.focus_settings_subpage("Config");
    let out = render(&mut app, 200, 50);
    assert!(out.contains("(missing)"), "expected (missing) annotation");
}
