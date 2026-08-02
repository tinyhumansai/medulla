//! Bracketed paste into the main TUI.
//!
//! A paste arrives as one `Event::Paste` rather than as a stream of synthetic
//! key presses, so the newlines inside it are text and not `Enter`. These tests
//! pin that down where it used to go wrong: a multi-line paste submitting itself
//! once per line, and a `/`-prefixed paste running whatever the command peek was
//! highlighting. Everything here is state-level — no real terminal is involved,
//! because the terminal mode itself is set once in `TermGuard::setup`.

use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use medulla::config::{LoadedConfig, TinyplaceConfig};
use medulla::runtime::mock::MockRuntime;
use medulla_tui::ui::app::{App, Cmd, TABS};

fn loaded() -> LoadedConfig {
    let mut l = LoadedConfig::defaults("medulla.tui.json".into());
    l.config.tinyplace = Some(TinyplaceConfig::default());
    l
}

/// An app on the Agents tab, where the chat composer lives.
fn agents_app() -> App {
    let mut app = App::new(Arc::new(MockRuntime::empty()), loaded());
    app.tab_index = tab("Agents");
    app
}

fn tab(name: &str) -> usize {
    TABS.iter().position(|t| *t == name).unwrap()
}

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn type_str(app: &mut App, text: &str) {
    for ch in text.chars() {
        let _ = app.on_event(key(KeyCode::Char(ch)));
    }
}

fn paste(app: &mut App, text: &str) -> Option<Cmd> {
    app.on_event(Event::Paste(text.into()))
}

#[test]
fn a_multiline_paste_lands_in_the_draft_instead_of_submitting_itself() {
    let mut app = agents_app();

    // The reported repro: a log excerpt with CRLF line endings and a trailing
    // newline. Every one of those breaks used to fire a separate submit.
    let cmd = paste(&mut app, "first\r\nsecond\rthird\n");

    assert!(cmd.is_none(), "a paste never produces a command");
    assert_eq!(app.draft_text(), "first\nsecond\nthird\n");
    assert_eq!(app.draft_cursor(), 19);
    assert_ne!(app.status(), "Cycle running…", "nothing was sent");
}

#[test]
fn only_an_explicit_enter_submits_what_was_pasted() {
    let mut app = agents_app();
    paste(&mut app, "look at\nthis trace");

    let cmd = app.on_event(key(KeyCode::Enter));

    assert!(
        matches!(cmd, Some(Cmd::Submit(ref s)) if s == "look at\nthis trace"),
        "the whole block goes as one instruction: {cmd:?}"
    );
    assert_eq!(app.draft_text(), "");
}

#[test]
fn a_paste_inserts_at_the_caret_rather_than_at_the_end() {
    let mut app = agents_app();
    type_str(&mut app, "ac");
    let _ = app.on_event(key(KeyCode::Left));

    let cmd = paste(&mut app, "b\n");

    assert!(cmd.is_none());
    assert_eq!(app.draft_text(), "ab\nc");
    assert_eq!(app.draft_cursor(), 3);
}

#[test]
fn a_paste_into_the_command_peek_does_not_run_the_highlighted_command() {
    let mut app = agents_app();
    type_str(&mut app, "/qu");
    assert!(
        app.selected_command().is_some(),
        "the peek is open on a match"
    );

    // `/qu` alone is not a command, but Enter here would have run `/quit`.
    let cmd = paste(&mut app, "estion for you\n");

    assert!(cmd.is_none());
    assert!(!app.should_quit, "the peeked command did not run");
    assert_eq!(app.draft_text(), "/question for you\n");
}

#[test]
fn a_paste_that_opens_the_peek_resets_its_cursor() {
    let mut app = agents_app();

    paste(&mut app, "/");

    assert_eq!(
        app.selected_command().as_deref(),
        Some("new"),
        "the peek opens on its first row, as it does when `/` is typed"
    );
}

#[test]
fn an_open_prompt_overlay_takes_the_paste_flattened_to_one_line() {
    let mut app = App::new(Arc::new(MockRuntime::empty()), loaded());
    app.tab_index = tab("Tasks");
    app.on_event(key(KeyCode::Enter));
    assert!(app.on_event(key(KeyCode::Char('a'))).is_none());
    assert!(app.prompt_state().is_some(), "`a` opens the title prompt");

    let cmd = paste(&mut app, "ship the\r\nfix\n");

    assert!(cmd.is_none(), "a paste never submits the prompt either");
    assert_eq!(
        app.prompt_state().map(|(_, text)| text),
        Some("ship the fix ".into()),
        "a single-line field has no second row to put a break on"
    );
}

#[test]
fn a_paste_outside_a_text_field_is_ignored() {
    let mut app = App::new(Arc::new(MockRuntime::empty()), loaded());
    app.tab_index = tab("Settings");

    assert!(paste(&mut app, "stray text").is_none());
    assert_eq!(
        app.draft_text(),
        "",
        "the Agents composer is not silently filled from another tab"
    );
}
