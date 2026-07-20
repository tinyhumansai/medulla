//! Feature tests for the Settings focus model: what `↑↓` do inside a focused
//! content pane on each subpage, which pages have nothing to scroll, and the
//! per-subpage refresh keys.
//!
//! The nav-vs-content split is covered from the other side in
//! `feature_settings.rs` (escaping the tab) and `feature_feedback_tab.rs`
//! (letters not firing from the nav). Here the concern is that entering a page
//! actually hands the arrow keys to *that page*.

use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::ContextItem;
use medulla_tui::ui::app::{App, Cmd};
use medulla_tui::ui::events::{NodeTrace, TuiEvent};

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

/// Render small enough that a paged list (Trace) actually changes when the
/// selection moves — a tall terminal fits every row and hides the scroll.
fn render(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(120, 16)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect()
}

/// An app with enough scripted state that every subpage has rows to scroll.
fn app_on(subpage: &str) -> App {
    let rt = Arc::new(MockRuntime::empty());
    let mut app = App::new(
        rt.clone(),
        LoadedConfig::defaults("medulla.tui.json".into()),
    );
    // Trace shows only `Trace` events, and pages by the visible height, so it
    // needs more of them than fit on screen before scrolling is observable.
    for i in 0..40 {
        rt.script_event(TuiEvent::Trace {
            entry: NodeTrace {
                node: format!("node-{i}"),
                ms: i,
                tool: None,
                op: Some("step".into()),
            },
        });
    }
    app.set_contexts(
        ["first", "second", "third"]
            .into_iter()
            .map(|name| ContextItem {
                ref_: name.into(),
                kind: "file".into(),
                bytes: name.len(),
                content: format!("contents of {name}"),
            })
            .collect(),
    );
    app.refresh_snapshot();
    let _ = app.focus_settings_subpage(subpage);
    app
}

#[test]
fn arrows_scroll_the_content_of_every_scrollable_subpage() {
    // Entering a page must hand `↑↓` to that page: the selection moves and the
    // subpage itself stays put. This is the whole point of the focus model.
    // Feedback's own arrow behaviour is covered in `feature_feedback_tab.rs`,
    // where a board is already seeded.
    for subpage in ["Appearance", "Config", "Trace", "Context"] {
        let mut app = app_on(subpage);
        assert!(app.settings_focused(), "{subpage} opens focused");
        let before = render(&mut app);

        app.on_event(key(KeyCode::Down));
        let after = render(&mut app);

        assert_eq!(
            app.settings_subpage(),
            subpage,
            "{subpage}: arrows must not move the nav while the page has focus"
        );
        assert_ne!(before, after, "{subpage}: the selection should have moved");

        // And back up again, returning to where it started.
        app.on_event(key(KeyCode::Up));
        assert_eq!(
            render(&mut app),
            before,
            "{subpage}: Up should undo the Down"
        );
    }
}

#[test]
fn arrows_are_swallowed_on_pages_with_nothing_to_scroll() {
    // Usage and Account have no list. The keys must still be consumed rather
    // than falling through to the global bindings, which would silently switch
    // tabs out from under the user.
    for subpage in ["Usage", "Account"] {
        let mut app = app_on(subpage);
        app.on_event(key(KeyCode::Down));
        assert_eq!(app.tab(), "Settings", "{subpage}: still on Settings");
        assert_eq!(
            app.settings_subpage(),
            subpage,
            "{subpage}: still on the same page"
        );
    }
}

#[test]
fn jk_still_browse_inside_a_focused_pane() {
    // The pre-focus-model muscle memory keeps working, so nobody has to relearn
    // navigation just because entry is now explicit.
    for subpage in ["Trace", "Context"] {
        let mut app = app_on(subpage);
        let before = render(&mut app);
        app.on_event(key(KeyCode::Char('j')));
        assert_ne!(render(&mut app), before, "{subpage}: j moves down");
        app.on_event(key(KeyCode::Char('k')));
        assert_eq!(render(&mut app), before, "{subpage}: k moves back up");
    }
}

#[test]
fn refresh_keys_emit_their_subpage_command() {
    let mut app = app_on("Context");
    let cmd = app.on_event(key(KeyCode::Char('r')));
    assert!(
        matches!(cmd, Some(Cmd::InspectContext)),
        "Context · r re-inspects"
    );
    assert!(app.status().contains("refreshing"), "{}", app.status());

    let mut app = app_on("Feedback");
    let cmd = app.on_event(key(KeyCode::Char('r')));
    assert!(
        matches!(cmd, Some(Cmd::LoadFeedback(_))),
        "Feedback · r reloads the board"
    );
    assert!(app.status().contains("refreshing"), "{}", app.status());
}

#[test]
fn appearance_arrows_pick_a_role_without_cycling_its_color() {
    // `↑↓` choose which role to edit; `←→`/Enter change it. Conflating the two
    // would make browsing the list mutate the theme.
    let mut app = app_on("Appearance");
    let primary = app.theme_primary();
    app.on_event(key(KeyCode::Down));
    assert_eq!(
        app.theme_primary(),
        primary,
        "moving the selection must not change any color"
    );
    // Now edit the role that is actually selected — not the primary.
    app.on_event(key(KeyCode::Enter));
    assert_eq!(
        app.theme_primary(),
        primary,
        "editing the second role leaves primary alone"
    );
}

#[test]
fn an_out_of_range_digit_is_not_claimed_by_settings() {
    // There are eight subpages; `9` names nothing, so Settings must decline it
    // rather than clamping to the last page.
    let mut app = app_on("Usage");
    app.on_event(key(KeyCode::Char('9')));
    assert_eq!(app.settings_subpage(), "Usage", "no jump happened");
}

#[test]
fn keys_a_subpage_does_not_bind_fall_through_to_the_global_bindings() {
    // Each subpage declines what it does not bind. Tab is the observable proof:
    // it reaches the global handler and switches tabs.
    for subpage in [
        "Usage",
        "Appearance",
        "Config",
        "Feedback",
        "Trace",
        "Context",
        "Account",
        "Help",
    ] {
        let mut app = app_on(subpage);
        app.on_event(key(KeyCode::Tab));
        assert_ne!(
            app.tab(),
            "Settings",
            "{subpage}: an unbound key must not be swallowed"
        );
    }
}

#[test]
fn appearance_k_moves_the_role_selection_back_up() {
    let mut app = app_on("Appearance");
    let before = render(&mut app);
    app.on_event(key(KeyCode::Char('j')));
    assert_ne!(render(&mut app), before, "j moves down");
    app.on_event(key(KeyCode::Char('k')));
    assert_eq!(render(&mut app), before, "k returns to the first role");
}
