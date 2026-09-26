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

/// The narrower marker is read by the workflow-surface question and not by the
/// whole-surface one — that difference is the point of having two.
#[test]
fn withholding_the_workflows_withholds_only_those() {
    let mut env = HashMap::new();

    withhold_workflows(&mut env);

    assert!(workflows_withheld(&env));
    assert!(
        !withheld(&env),
        "the rest of the Medulla surface stays with this launch"
    );
}

/// Withholding everything withholds the workflow tools too. Asserted because
/// the two markers are checked by different seams — the skills renderer asks
/// the narrow question, and it must still skip a launch getting no tools at
/// all.
#[test]
fn withholding_everything_also_withholds_the_workflows() {
    let mut env = HashMap::new();

    withhold(&mut env);

    assert!(workflows_withheld(&env));
    assert!(withheld(&env));
}

/// Unmarked and unrecognised environments both describe an ordinary launch.
#[test]
fn only_the_two_markers_withhold_the_workflow_surface() {
    assert!(!workflows_withheld(&HashMap::new()));
    let mut env = HashMap::new();
    env.insert(HARNESS_TOOLS_ENV.to_string(), "full".to_string());
    assert!(!workflows_withheld(&env));
}

/// A tool mode selects a slice of the workflow family, so it cannot outlive
/// the family. The fleet grant is left alone deliberately: this withholding is
/// scoped to the workflow surface.
#[test]
fn withholding_the_workflows_clears_the_mode_but_not_the_fleet_grant() {
    let mut env = HashMap::new();
    env.insert(
        crate::control_socket::MCP_PARENT_GRANT_ENV.to_string(),
        "parent-token".to_string(),
    );
    #[cfg(feature = "workflows")]
    env.insert(crate::mcp::TOOL_MODE_ENV.to_string(), "full".to_string());

    withhold_workflows(&mut env);

    #[cfg(feature = "workflows")]
    assert!(!env.contains_key(crate::mcp::TOOL_MODE_ENV));
    assert_eq!(
        env.get(crate::control_socket::MCP_PARENT_GRANT_ENV)
            .map(String::as_str),
        Some("parent-token"),
    );
}
