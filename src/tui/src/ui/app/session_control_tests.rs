//! Focused tests for harness-picker keyboard classification.

use crossterm::event::KeyModifiers;

use super::session_control::is_text_input;

#[test]
fn workspace_text_accepts_altgr_but_rejects_control_shortcuts() {
    assert!(is_text_input(KeyModifiers::NONE));
    assert!(is_text_input(KeyModifiers::SHIFT));
    assert!(is_text_input(KeyModifiers::CONTROL | KeyModifiers::ALT));
    assert!(!is_text_input(KeyModifiers::CONTROL));
    assert!(!is_text_input(KeyModifiers::ALT));
}
