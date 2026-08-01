//! The slash-command peek: typing `/` in the composer offers the catalog,
//! narrows it as you type, completes on Tab, and runs the highlighted command on
//! Enter — plus the simple commands themselves.

use std::sync::Arc;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use medulla::config::LoadedConfig;
use medulla::runtime::mock::MockRuntime;
use medulla::runtime::Runtime;
use medulla_tui::ui::app::{App, Cmd, TABS};

fn agents_app() -> App {
    let runtime: Arc<dyn Runtime> = Arc::new(MockRuntime::demo());
    let mut app = App::new(runtime, LoadedConfig::defaults("medulla.tui.json".into()));
    app.tab_index = TABS.iter().position(|t| *t == "Agents").unwrap();
    app
}

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn type_str(app: &mut App, text: &str) {
    for ch in text.chars() {
        let _ = app.on_event(key(KeyCode::Char(ch)));
    }
}

fn render(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| app.draw(f)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

#[test]
fn a_slash_opens_the_catalog_with_descriptions() {
    let mut app = agents_app();
    // Nothing is offered until a command is being typed.
    let out = render(&mut app, 140, 44);
    assert!(!out.contains("Commands"), "{out}");

    type_str(&mut app, "/");
    let out = render(&mut app, 140, 44);
    assert!(out.contains("Commands"), "the peek opens: {out}");
    assert!(out.contains("/new"), "{out}");
    assert!(out.contains("Open a new thread"), "descriptions too: {out}");

    // The list is capped so it cannot swallow the transcript above it; the tail
    // is reached by scrolling rather than by growing the popup.
    assert!(!out.contains("/quit"), "capped: {out}");
    // Derived from the catalog, not a magic number: the selection wraps, so a
    // hardcoded count silently lands back at the top when a command is added or
    // removed — which is a pass-shaped failure until the assertion below.
    for _ in 0..medulla::ui::command::COMMANDS.len() - 1 {
        let _ = app.on_event(key(KeyCode::Down));
    }
    let out = render(&mut app, 140, 44);
    assert!(
        out.contains("/quit"),
        "the selection scrolls the window: {out}"
    );
    assert!(out.contains("Exit medulla"), "{out}");
}

#[test]
fn typing_narrows_the_list_and_an_unknown_prefix_says_so() {
    let mut app = agents_app();
    type_str(&mut app, "/cl");
    let out = render(&mut app, 140, 44);
    assert!(out.contains("/clear"), "{out}");
    assert!(!out.contains("/quit"), "narrowed away: {out}");

    type_str(&mut app, "zzz");
    let out = render(&mut app, 140, 44);
    assert!(out.contains("no command matches"), "{out}");
}

#[test]
fn tab_completes_the_highlighted_command() {
    let mut app = agents_app();
    type_str(&mut app, "/qu");
    let _ = app.on_event(key(KeyCode::Tab));
    assert_eq!(app.draft_text(), "/quit");

    // A command that takes an argument completes with the space already typed,
    // which also closes the peek — the choice has been made.
    let mut with_arg = agents_app();
    let app = &mut with_arg;
    type_str(app, "/cop");
    let _ = app.on_event(key(KeyCode::Tab));
    assert_eq!(app.draft_text(), "/copy ");
    let out = render(app, 140, 44);
    assert!(!out.contains("Copy the transcript"), "peek closed: {out}");
}

#[test]
fn arrows_move_the_peek_rather_than_the_rail() {
    let mut app = agents_app();
    type_str(&mut app, "/");
    let first = app.selected_command().expect("a command is highlighted");
    let _ = app.on_event(key(KeyCode::Down));
    let second = app.selected_command().expect("still highlighted");
    assert_ne!(first, second, "Down walks the list");

    // The list wraps, so a short catalog is quick to reach the end of.
    let _ = app.on_event(key(KeyCode::Up));
    assert_eq!(app.selected_command().as_deref(), Some(first.as_str()));
}

#[test]
fn enter_runs_the_highlighted_command_not_the_typed_prefix() {
    let mut app = agents_app();
    // `/qu` alone is not a command; picking `/quit` from the peek must quit.
    type_str(&mut app, "/qu");
    let _ = app.on_event(key(KeyCode::Enter));
    assert!(app.should_quit, "the highlighted command ran");
}

#[test]
fn esc_dismisses_the_peek_by_clearing_the_draft() {
    let mut app = agents_app();
    type_str(&mut app, "/set");
    let _ = app.on_event(key(KeyCode::Esc));
    assert_eq!(app.draft_text(), "");
    let out = render(&mut app, 140, 44);
    assert!(!out.contains("Commands"), "{out}");
}

#[test]
fn the_simple_commands_do_what_they_say() {
    let mut app = agents_app();
    type_str(&mut app, "/clear");
    let _ = app.on_event(key(KeyCode::Enter));
    assert!(app.status().contains("View reset"), "{}", app.status());

    type_str(&mut app, "/new");
    let _ = app.on_event(key(KeyCode::Enter));
    assert!(app.status().contains("Opened"), "{}", app.status());

    type_str(&mut app, "/exit");
    let _ = app.on_event(key(KeyCode::Enter));
    assert!(app.should_quit, "/exit is a spelling of /quit");
}

#[test]
fn help_lists_every_command_from_the_catalog() {
    let mut app = agents_app();
    type_str(&mut app, "/help");
    let _ = app.on_event(key(KeyCode::Enter));
    assert_eq!(app.settings_subpage(), "Help");
    // Tall enough to hold the whole page. Help outgrew a short terminal once the
    // harness bindings landed on it, so it scrolls now (see below) — what this
    // asserts is that every command is *on* the page, not that the page fits in
    // any particular window.
    let out = render(&mut app, 160, 64);
    for spec in medulla::ui::command::COMMANDS {
        assert!(out.contains(spec.name), "help omits /{}: {out}", spec.name);
    }
}

#[test]
fn help_scrolls_so_a_short_terminal_can_reach_the_commands() {
    // The page is longer than a small window, and a keyboard reference whose
    // bottom half is unreachable is not one.
    let mut app = agents_app();
    type_str(&mut app, "/help");
    let _ = app.on_event(key(KeyCode::Enter));

    let top = render(&mut app, 160, 40);
    assert!(top.contains("Tab / Shift-Tab switch views"), "{top}");
    assert!(
        !top.contains("/handoff"),
        "the tail starts off-screen: {top}"
    );

    // Step into the content pane, then walk down to the end.
    let _ = app.on_event(key(KeyCode::Enter));
    for _ in 0..60 {
        let _ = app.on_event(key(KeyCode::Down));
    }
    let bottom = render(&mut app, 160, 40);
    assert!(
        bottom.contains("/handoff"),
        "scrolling must reach the command list: {bottom}"
    );
}

#[test]
fn help_scroll_bound_includes_rows_wrapped_by_a_narrow_terminal() {
    let mut app = agents_app();
    type_str(&mut app, "/help");
    let _ = app.on_event(key(KeyCode::Enter));

    let top = render(&mut app, 72, 40);
    assert!(
        !top.contains("/handoff"),
        "the tail starts off-screen: {top}"
    );

    let _ = app.on_event(key(KeyCode::Enter));
    for _ in 0..100 {
        let _ = app.on_event(key(KeyCode::Down));
    }
    let bottom = render(&mut app, 72, 40);
    assert!(
        bottom.contains("/handoff"),
        "wrapped rows must contribute to the scroll bound: {bottom}"
    );
}

#[test]
fn an_ordinary_message_is_never_treated_as_a_command() {
    let mut app = agents_app();
    type_str(&mut app, "and/or maybe");
    let out = render(&mut app, 140, 44);
    assert!(!out.contains("Commands"), "a mid-line slash is text: {out}");
    match app.on_event(key(KeyCode::Enter)) {
        Some(Cmd::Submit(text)) => assert_eq!(text, "and/or maybe"),
        other => panic!("expected a plain submit, got {other:?}"),
    }
}
