//! Paste onto a tab with no text field, and into the one single-line overlay
//! that still takes one.
//!
//! The Agents composer this file was written for is gone with the orchestrator
//! it submitted to, and with it every case about multi-line drafts, the command
//! peek, and moving focus in on a paste. What is left is the rule those cases
//! existed to protect: a payload never lands anywhere the operator cannot see
//! it. The multi-line path is still covered against the Workflows copilot, in
//! [`super::copilot`].

use crate::helpers::*;

#[test]
fn an_open_prompt_overlay_takes_the_paste_flattened_to_one_line() {
    let mut app = App::new(Arc::new(MockRuntime::empty()), loaded());
    app.tab_index = tab_index("Hosts");
    app.focus_routing_subpage("Add Host");
    // One confirm opens the address prompt, which is the single-line field here.
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
        "no draft is silently filled from a tab with no field"
    );
}

#[test]
fn a_paste_on_the_agents_tab_is_dropped_rather_than_banked() {
    // The tab is a list now: every row on it is something already running, and
    // there is nowhere on screen for a payload to appear. Retaining it in a
    // hidden draft is exactly what the old composer refusals existed to stop.
    let mut app = demo_agents_app();

    assert!(paste(&mut app, "a whole log excerpt\n").is_none());

    assert_eq!(
        app.draft_text(),
        "",
        "nothing was retained in a draft nobody can see"
    );
}
