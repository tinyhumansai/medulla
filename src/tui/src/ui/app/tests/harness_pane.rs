//! Regression tests for harness-pane-only keyboard commands.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use std::sync::Arc;

use super::super::types::{tab_pos, App};

/// Build the standard app fixture with the harness pane available.
fn app() -> App {
    let runtime = Arc::new(MockRuntime::demo());
    let mut loaded = LoadedConfig::defaults("medulla.tui.json".into());
    loaded.config.link = Some(medulla::config::LinkConfig::default());
    App::new(runtime, loaded)
}

/// Find a tab by its visible label so the test survives tab reordering.
fn tab(name: &str) -> usize {
    tab_pos(name)
}

#[test]
fn k_on_a_harness_this_device_does_not_host_arms_nothing() {
    // The fixture names a pane session but hosts no sessions at all, so the row
    // resolves to nothing: not running, and no local record to release either.
    // Both halves of `k` decline, and the key stays a command rather than
    // falling through to the composer.
    let mut app = app();
    app.tab_index = tab("Sessions");
    app.pane_session = Some("selected-harness".to_owned());

    let cmd = app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));

    assert!(cmd.is_none());
    assert!(app.harness_close_armed.is_none());
    assert!(app.status().contains("not hosting"), "{}", app.status());
    assert_eq!(app.draft.text, "", "the shortcut must not type into chat");
}

#[test]
fn k_without_a_selected_harness_arms_nothing() {
    let mut app = app();
    app.tab_index = tab("Sessions");

    app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));

    assert!(
        app.harness_close_armed.is_none(),
        "there is no harness under the cursor to close"
    );
}

#[test]
fn an_armed_harness_close_takes_only_y() {
    let mut app = app();
    app.tab_index = tab("Sessions");
    app.arm_harness_close("selected-harness".to_owned());

    app.on_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));

    assert!(app.harness_close_armed.is_none());
    assert!(app.status().contains("cancelled"), "{}", app.status());
}

#[test]
fn d_without_a_selected_harness_opens_no_diff() {
    let mut app = app();
    app.tab_index = tab("Sessions");

    app.on_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));

    assert_eq!(app.tab(), "Sessions", "the key must not navigate anywhere");
    assert_eq!(
        app.pane_view,
        super::super::types::PaneView::Harness,
        "with no session under the cursor there is no diff to swap to"
    );
}
