//! Tests for the main module.

use super::daemon_uses_tui;

#[test]
fn daemon_defaults_to_the_tui_only_on_a_terminal() {
    assert!(daemon_uses_tui(true, &["daemon".into()]));
    assert!(!daemon_uses_tui(false, &["daemon".into()]));
    assert!(!daemon_uses_tui(
        true,
        &["daemon".into(), "--headless".into()]
    ));
}
