//! Compatibility tests for additive worker capability fields.

use crate::protocol::{parse_agent_capabilities, AgentCapabilities, BudgetWindow};

#[test]
fn budget_window_defaults_to_unknown() {
    assert_eq!(BudgetWindow::default(), BudgetWindow::Unknown);
}

#[test]
fn screen_kill_support_is_additive_and_defaults_off_for_older_workers() {
    let older = parse_agent_capabilities(r#"{"providers":["claude"]}"#).unwrap();
    assert!(!older.screen_kill);

    let current = AgentCapabilities {
        screen_kill: true,
        ..Default::default()
    };
    let value = serde_json::to_value(&current).unwrap();
    assert_eq!(value["screenKill"], true);
}
