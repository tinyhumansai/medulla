//! Focused tests for the reusable multi-pane navigation state machine.

use crossterm::event::KeyCode;

use super::{navigate, NavAction};

#[test]
fn menu_arrows_move_selection_without_entering_content() {
    let mut selected = 0;
    let mut focused = false;
    assert_eq!(
        navigate(KeyCode::Down, 4, &mut selected, &mut focused, true),
        NavAction::SelectionChanged
    );
    assert_eq!(selected, 1);
    assert!(!focused);
}

#[test]
fn digits_jump_directly_into_a_page() {
    let mut selected = 0;
    let mut focused = false;
    assert_eq!(
        navigate(KeyCode::Char('4'), 4, &mut selected, &mut focused, true),
        NavAction::SelectionChanged
    );
    assert_eq!(selected, 3);
    assert!(focused);
}

#[test]
fn content_escape_respects_a_page_owned_confirmation() {
    let mut selected = 2;
    let mut focused = true;
    assert_eq!(
        navigate(KeyCode::Esc, 4, &mut selected, &mut focused, false),
        NavAction::Unhandled
    );
    assert!(focused);
    assert_eq!(
        navigate(KeyCode::Esc, 4, &mut selected, &mut focused, true),
        NavAction::Left
    );
    assert!(!focused);
}

#[test]
fn structural_keys_are_left_for_the_top_level_app() {
    let mut selected = 0;
    let mut focused = false;
    assert_eq!(
        navigate(KeyCode::Tab, 4, &mut selected, &mut focused, true),
        NavAction::Unhandled
    );
}
