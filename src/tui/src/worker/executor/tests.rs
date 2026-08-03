//! Focused tests for PTY executor lifecycle decisions.

use medulla::sessions::SessionClass;

use super::run::retains_workspace_context;
use crate::worker::pty::HarnessControl;

#[test]
fn mapper_context_survives_only_for_live_reusable_sessions() {
    assert!(retains_workspace_context(
        SessionClass::Unbound,
        Some(HarnessControl::Orchestrator),
        true,
    ));
    assert!(retains_workspace_context(
        SessionClass::Bounded,
        Some(HarnessControl::User),
        true,
    ));
    assert!(!retains_workspace_context(
        SessionClass::Bounded,
        Some(HarnessControl::Orchestrator),
        true,
    ));
    assert!(!retains_workspace_context(
        SessionClass::Unbound,
        Some(HarnessControl::Orchestrator),
        false,
    ));
}
