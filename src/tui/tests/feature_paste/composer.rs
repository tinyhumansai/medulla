//! Paste into the Agents composer: the base case, and the refusals that keep a
//! payload out of a draft nobody can see.

use crate::helpers::*;

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
    app.tab_index = tab_index("Hosts");
    app.focus_routing_subpage("Add Host");
    // Local leads the kind picker, so a remote add is Down then two confirms:
    // the second opens the address prompt, which is the single-line field here.
    let _ = app.on_event(key(KeyCode::Down));
    let _ = app.on_event(key(KeyCode::Char('a')));
    let _ = app.on_event(key(KeyCode::Char('a')));
    assert!(app.prompt_state().is_some(), "the address prompt is open");

    let cmd = paste(&mut app, "host:1234\r\nmy label\n");

    assert!(cmd.is_none(), "a paste never submits the prompt either");
    assert_eq!(
        app.prompt_state().map(|(_, text)| text),
        Some("host:1234 my label ".into()),
        "a single-line field has no second row to put a break on"
    );
}

#[test]
fn a_paste_outside_a_text_field_is_ignored() {
    let mut app = App::new(Arc::new(MockRuntime::empty()), loaded());
    app.tab_index = tab_index("Settings");

    assert!(paste(&mut app, "stray text").is_none());
    assert_eq!(
        app.draft_text(),
        "",
        "the Agents composer is not silently filled from another tab"
    );
}

#[test]
fn a_paste_is_refused_when_no_composer_is_on_screen() {
    // The composer is drawn for the orchestrator lane and nowhere else, which is
    // why typing on any other rail row stays on the rail. A paste that ignored
    // that filled an invisible draft the operator could later submit by accident
    // — the same failure the keyboard already closed.
    let mut app = demo_agents_app();
    key_press(&mut app, KeyCode::Esc);
    key_press(&mut app, KeyCode::Down);
    assert!(
        !app.agents_composer_shown(),
        "the cursor is on a row that draws no composer"
    );

    assert!(paste(&mut app, "a whole log excerpt\n").is_none());

    assert_eq!(
        app.draft_text(),
        "",
        "nothing was retained in a draft nobody can see"
    );
    assert!(
        app.status().contains("orchestrator lane"),
        "the refusal says what to do about it: {}",
        app.status()
    );
}

#[test]
fn a_paste_from_the_rail_moves_the_keyboard_to_the_composer() {
    // Esc on an empty draft parks focus on the rail with the cursor still on the
    // orchestrator, so the composer is visible but not driving the keyboard.
    // Typing there returns focus with the character; a paste that inserted
    // without moving focus left Enter merely stepping back in rather than
    // sending, and the arrows still walking lanes instead of the pasted text.
    let mut app = demo_agents_app();
    key_press(&mut app, KeyCode::Esc);
    assert!(app.agents_rail_focused(), "the rail has the keyboard");
    assert!(app.agents_composer_shown(), "and the composer is on screen");

    assert!(paste(&mut app, "ship it").is_none());

    assert_eq!(app.draft_text(), "ship it");
    assert!(
        !app.agents_rail_focused(),
        "the keyboard followed the paste into the composer"
    );
    let cmd = app.on_event(key(KeyCode::Enter));
    assert!(
        matches!(cmd, Some(Cmd::Submit(ref sent)) if sent == "ship it"),
        "so the next Enter sends it: {cmd:?}"
    );
}
