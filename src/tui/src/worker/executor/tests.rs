//! Focused tests for PTY executor lifecycle decisions.

use medulla::sessions::SessionClass;

use std::collections::HashMap;

use super::run::{retains_workspace_context, retire_stopped_workspace_context};
use crate::worker::pty::SessionControl;

#[test]
fn mapper_context_survives_only_for_live_reusable_sessions() {
    assert!(retains_workspace_context(
        SessionClass::Unbound,
        Some(SessionControl::Orchestrator),
        true,
    ));
    assert!(retains_workspace_context(
        SessionClass::Bounded,
        Some(SessionControl::User),
        true,
    ));
    assert!(!retains_workspace_context(
        SessionClass::Bounded,
        Some(SessionControl::Orchestrator),
        true,
    ));
    assert!(!retains_workspace_context(
        SessionClass::Unbound,
        Some(SessionControl::Orchestrator),
        false,
    ));
}

#[test]
fn a_failed_orchestrator_stop_keeps_operator_owned_mapper_context() {
    let mut context = HashMap::from([(
        "pty-1".to_string(),
        (
            Some("/repo/worktrees/fix".to_string()),
            Some("fix".to_string()),
            Some("https://github.com/acme/repo/pull/42".to_string()),
        ),
    )]);
    retire_stopped_workspace_context(&mut context, "pty-1", false);
    assert!(context.contains_key("pty-1"));

    retire_stopped_workspace_context(&mut context, "pty-1", true);
    assert!(!context.contains_key("pty-1"));
}
