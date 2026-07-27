//! Tests for the copilot transcript model.

use super::*;

#[test]
fn asking_records_the_instruction_and_marks_the_thread_busy() {
    let mut state = CopilotState::new("sweep");

    state.ask("add a slack step");

    assert_eq!(state.turns[0].role, TurnRole::User);
    assert_eq!(state.turns[0].text, "add a slack step");
    assert!(state.busy);
}

#[test]
fn a_reply_ends_the_turn() {
    let mut state = CopilotState::new("sweep");
    state.ask("go");

    state.reply("added it");

    assert!(!state.busy);
    assert_eq!(state.turns.last().unwrap().role, TurnRole::Agent);
}

#[test]
fn an_empty_reply_ends_the_turn_without_an_empty_line() {
    let mut state = CopilotState::new("sweep");
    state.ask("go");

    state.reply("   ");

    assert!(!state.busy);
    assert_eq!(
        state.turns.len(),
        1,
        "nothing was said, so nothing is shown"
    );
}

#[test]
fn a_failure_ends_the_turn_and_says_why() {
    let mut state = CopilotState::new("sweep");
    state.ask("go");

    state.failed("no harness installed");

    assert!(!state.busy);
    assert_eq!(state.turns.last().unwrap().role, TurnRole::Error);
}

#[test]
fn a_repeated_status_line_is_not_shown_twice_in_a_row() {
    let mut state = CopilotState::new("sweep");

    state.status("thinking");
    state.status("thinking");

    assert_eq!(state.turns.len(), 1);
}

#[test]
fn the_same_status_after_something_else_is_shown_again() {
    let mut state = CopilotState::new("sweep");

    state.status("thinking");
    state.reply("done");
    state.status("thinking");

    assert_eq!(state.turns.len(), 3);
}

#[test]
fn status_chatter_is_trimmed_but_the_conversation_is_not() {
    let mut state = CopilotState::new("sweep");
    state.ask("go");
    for index in 0..200 {
        state.status(format!("step {index}"));
    }
    state.reply("done");

    let statuses = state
        .turns
        .iter()
        .filter(|turn| turn.role == TurnRole::Status)
        .count();
    assert!(statuses <= 40, "{statuses} status lines survived");
    assert_eq!(state.turns.first().unwrap().role, TurnRole::User);
    assert_eq!(state.turns.last().unwrap().role, TurnRole::Agent);
    // The trim drops the *oldest* chatter, so the newest is what is on screen.
    assert!(state.turns.iter().any(|turn| turn.text == "step 199"));
}

#[test]
fn changes_are_their_own_kind_of_line() {
    let mut state = CopilotState::new("sweep");

    state.changed(["+ node notify (tool_call)".to_string()]);

    assert_eq!(state.turns[0].role, TurnRole::Change);
}

#[test]
fn every_role_has_a_glyph_and_a_colour_and_only_status_is_dim() {
    for role in [
        TurnRole::User,
        TurnRole::Agent,
        TurnRole::Status,
        TurnRole::Change,
        TurnRole::Error,
    ] {
        assert!(!role.glyph().is_empty(), "{role:?}");
        assert!(!role.color().is_empty(), "{role:?}");
        assert_eq!(role.dim(), role == TurnRole::Status, "{role:?}");
    }
}
