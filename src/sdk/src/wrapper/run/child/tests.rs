//! Unit tests for the child-specific environment isolation policy.

use std::collections::HashMap;

use super::core_state_vars_to_remove;
use crate::protocol::HarnessProvider;
use crate::wrapper::WrapperConfig;

/// Build the minimal wrapper configuration needed to select a provider.
fn config(provider: HarnessProvider) -> WrapperConfig {
    WrapperConfig {
        provider,
        child_args: Vec::new(),
        env: HashMap::new(),
        cwd: "/".to_string(),
        no_bridge: true,
        session_id: None,
        pty_spawner: None,
        attribution: false,
        hooks: crate::harness_hooks::HooksConfig::default(),
    }
}

#[test]
fn core_workspace_is_removed_only_for_external_harnesses() {
    assert_eq!(
        core_state_vars_to_remove(&config(HarnessProvider::Codex)),
        vec!["OPENHUMAN_WORKSPACE"]
    );
    assert!(core_state_vars_to_remove(&config(HarnessProvider::Openhuman)).is_empty());
}
