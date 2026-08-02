//! Unit tests for task-specific harness environment construction.

#[cfg(feature = "workflows")]
#[test]
fn clears_inherited_mode_transport_and_fleet_capabilities() {
    let env = std::collections::HashMap::from([
        (crate::mcp::TOOL_MODE_ENV.to_string(), "propose".to_string()),
        (
            crate::daemon::providers::HARNESS_PROTOCOL_ENV.to_string(),
            "acp".to_string(),
        ),
        (
            crate::control_socket::MCP_SOCKET_ENV.to_string(),
            "/tmp/parent.sock".to_string(),
        ),
        (
            crate::control_socket::MCP_GRANT_ENV.to_string(),
            "parent-grant".to_string(),
        ),
    ]);

    let env = super::with_tool_mode_at_depth(env, None, 0);

    assert!(!env.contains_key(crate::mcp::TOOL_MODE_ENV));
    assert!(!env.contains_key(crate::control_socket::MCP_SOCKET_ENV));
    assert!(!env.contains_key(crate::control_socket::MCP_GRANT_ENV));
    assert!(!env.contains_key(crate::daemon::providers::HARNESS_PROTOCOL_ENV));
}

#[cfg(feature = "workflows")]
#[test]
fn tool_mode_forces_acp_transport() {
    let env = super::with_tool_mode_at_depth(std::collections::HashMap::new(), Some("propose"), 0);

    assert_eq!(
        env.get(crate::daemon::providers::HARNESS_PROTOCOL_ENV)
            .map(String::as_str),
        Some("acp")
    );
}

#[cfg(feature = "workflows")]
#[test]
fn delegated_full_mode_task_on_remote_worker_keeps_provider_transport() {
    let env = super::with_tool_mode_at_depth(std::collections::HashMap::new(), None, 1);

    assert!(!env.contains_key(crate::daemon::providers::HARNESS_PROTOCOL_ENV));
    assert!(!env.contains_key(crate::mcp::TOOL_MODE_ENV));
}

#[cfg(feature = "workflows")]
#[test]
fn a_verified_parent_handoff_forces_acp_without_reusing_the_parent_as_the_child() {
    let env = std::collections::HashMap::from([
        (
            crate::control_socket::MCP_PARENT_SOCKET_ENV.to_string(),
            "/tmp/parent.sock".to_string(),
        ),
        (
            crate::control_socket::MCP_PARENT_GRANT_ENV.to_string(),
            "parent-grant".to_string(),
        ),
    ]);

    let env = super::with_tool_mode_at_depth(env, None, 1);

    assert_eq!(
        env.get(crate::daemon::providers::HARNESS_PROTOCOL_ENV)
            .map(String::as_str),
        Some("acp")
    );
    assert!(!env.contains_key(crate::control_socket::MCP_SOCKET_ENV));
    assert!(!env.contains_key(crate::control_socket::MCP_GRANT_ENV));
}

#[cfg(feature = "workflows")]
#[test]
fn root_and_delegated_tasks_use_acp_when_process_owns_fleet_plane() {
    assert!(super::task_can_reach_fleet(true));
    assert!(!super::task_can_reach_fleet(false));
}
