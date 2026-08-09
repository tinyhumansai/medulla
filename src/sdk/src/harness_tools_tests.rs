//! Unit tests for the tool-withholding marker.

use std::collections::HashMap;

use super::*;

/// An environment nobody marked describes an ordinary launch. Asserted
/// explicitly because the default has to be "tools", not "no tools": a seam
/// that failed open the other way would silently strip every operator session.
#[test]
fn an_unmarked_environment_is_not_withheld() {
    assert!(!withheld(&HashMap::new()));
}

/// Only the one value withholds. A future build writing something else here
/// must not be read by this one as a withholding.
#[test]
fn only_the_withheld_value_withholds() {
    let mut env = HashMap::new();
    env.insert(HARNESS_TOOLS_ENV.to_string(), "full".to_string());
    assert!(!withheld(&env));
    env.insert(HARNESS_TOOLS_ENV.to_string(), WITHHELD.to_string());
    assert!(withheld(&env));
}

/// The marker is not enough on its own: an inherited grant would let a nested
/// launch exchange its way back to a tool surface underneath it.
#[test]
fn withholding_clears_every_inherited_grant() {
    let mut env = HashMap::new();
    env.insert(
        crate::control_socket::MCP_SOCKET_ENV.to_string(),
        "/tmp/sock".to_string(),
    );
    env.insert(
        crate::control_socket::MCP_GRANT_ENV.to_string(),
        "token".to_string(),
    );
    env.insert(
        crate::control_socket::MCP_PARENT_SOCKET_ENV.to_string(),
        "/tmp/sock".to_string(),
    );
    env.insert(
        crate::control_socket::MCP_PARENT_GRANT_ENV.to_string(),
        "parent-token".to_string(),
    );
    #[cfg(feature = "workflows")]
    env.insert(crate::mcp::TOOL_MODE_ENV.to_string(), "full".to_string());

    withhold(&mut env);

    assert!(withheld(&env));
    assert!(!env.contains_key(crate::control_socket::MCP_SOCKET_ENV));
    assert!(!env.contains_key(crate::control_socket::MCP_GRANT_ENV));
    assert!(!env.contains_key(crate::control_socket::MCP_PARENT_SOCKET_ENV));
    assert!(!env.contains_key(crate::control_socket::MCP_PARENT_GRANT_ENV));
    #[cfg(feature = "workflows")]
    assert!(!env.contains_key(crate::mcp::TOOL_MODE_ENV));
}

/// Unrelated environment is left alone — the child still needs its PATH, its
/// workspace, and whatever the operator configured.
#[test]
fn withholding_touches_nothing_else() {
    let mut env = HashMap::new();
    env.insert("PATH".to_string(), "/usr/bin".to_string());
    env.insert("GH_REPO".to_string(), "owner/name".to_string());

    withhold(&mut env);

    assert_eq!(env.get("PATH").map(String::as_str), Some("/usr/bin"));
    assert_eq!(env.get("GH_REPO").map(String::as_str), Some("owner/name"));
}
