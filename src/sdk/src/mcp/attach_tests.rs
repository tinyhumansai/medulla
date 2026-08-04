//! Unit tests for the shared harness attachment policy.
//!
//! The control plane is a process-wide `OnceLock`, so these tests never install
//! one: they cover the decisions that hold with or without a fleet, and the
//! grant-carrying paths are exercised end to end in
//! `src/sdk/tests/feature_mcp_fleet.rs` where a real socket is bound.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value;

use super::*;

/// A spec with no fleet grant, as a host with workflows on and no plane builds.
fn spec_without_fleet() -> ServerSpec {
    server_spec(None, None, true).expect("workflows on is enough to serve a server")
}

#[test]
fn a_host_with_neither_family_attaches_nothing() {
    assert!(
        server_spec(None, None, false).is_none(),
        "workflows off and no fleet grant leaves nothing worth attaching"
    );
}

#[test]
fn a_fleet_grant_alone_is_enough_to_attach() {
    let spec = server_spec(
        None,
        Some((PathBuf::from("/run/medulla.sock"), "tok".to_string())),
        false,
    )
    .expect("a fleet grant is a family, even with workflow authoring off");
    assert_eq!(spec.args, vec!["mcp".to_string()]);
}

#[test]
fn the_grant_never_reaches_the_non_secret_environment() {
    let spec = server_spec(
        Some("run"),
        Some((PathBuf::from("/run/medulla.sock"), "s3cret".to_string())),
        true,
    )
    .expect("a server is served here");

    let rendered = spec.claude_mcp_config();
    assert!(
        !rendered.contains("s3cret"),
        "the bearer token must never be rendered into an argv registration: {rendered}"
    );
    assert!(
        spec.secret_env
            .iter()
            .any(|(key, value)| key == crate::control_socket::MCP_GRANT_ENV && value == "s3cret"),
        "the token has to reach the server somehow — via write_config_file's \
         owner-only file, not the argv document"
    );
    assert!(spec
        .secret_env
        .iter()
        .any(|(key, _)| key == crate::control_socket::MCP_SOCKET_ENV));
}

#[test]
fn the_tool_mode_and_its_scope_are_split_apart() {
    let spec = server_spec(Some("propose:nightly-sweep"), None, true).expect("a server is served");
    assert_eq!(
        spec.env,
        vec![
            (
                super::super::TOOL_MODE_ENV.to_string(),
                "propose".to_string()
            ),
            (
                super::super::TOOL_SCOPE_ENV.to_string(),
                "nightly-sweep".to_string()
            ),
        ]
    );
}

#[test]
fn an_unset_tool_mode_leaves_the_server_its_own_default() {
    assert!(
        spec_without_fleet().env.is_empty(),
        "nothing to say about the mode is not the same as saying `full`"
    );
}

#[test]
fn the_claude_registration_names_this_binary_and_the_mcp_verb() {
    let spec = spec_without_fleet();
    let document: Value = serde_json::from_str(&spec.claude_mcp_config()).expect("a JSON document");
    let entry = &document["mcpServers"]["medulla"];
    assert_eq!(entry["command"], spec.command.display().to_string());
    assert_eq!(entry["args"][0], "mcp");
    assert!(
        entry.get("env").is_none(),
        "an empty env block is noise in a registration"
    );
}

#[test]
fn the_claude_registration_carries_a_tool_mode_that_is_not_secret() {
    let spec = server_spec(Some("run"), None, true).expect("a server is served");
    let document: Value = serde_json::from_str(&spec.claude_mcp_config()).expect("a JSON document");
    assert_eq!(
        document["mcpServers"]["medulla"]["env"][super::super::TOOL_MODE_ENV],
        "run"
    );
}

#[test]
fn only_a_provider_with_a_verified_flag_is_attached_on_its_argv() {
    assert!(supports_cli_attach(
        crate::protocol::HarnessProvider::Claude
    ));
    for provider in [
        crate::protocol::HarnessProvider::Codex,
        crate::protocol::HarnessProvider::Opencode,
        crate::protocol::HarnessProvider::Openhuman,
    ] {
        assert!(
            !supports_cli_attach(provider),
            "{provider:?} has no registration flag we have verified"
        );
    }
}

#[test]
fn attaching_a_cli_registers_the_server_on_the_argv() {
    let mut env = HashMap::new();
    let mut args = vec!["--resume".to_string()];
    attach_cli(
        crate::protocol::HarnessProvider::Claude,
        "pty-1",
        &mut env,
        &mut args,
    );

    assert_eq!(args[0], "--resume", "the caller's own argv is preserved");
    let flag = args
        .iter()
        .position(|arg| arg == "--mcp-config")
        .expect("claude is registered through --mcp-config");
    let document: Value = serde_json::from_str(&args[flag + 1]).expect("a JSON document");
    assert_eq!(document["mcpServers"]["medulla"]["args"][0], "mcp");
}

#[test]
fn attaching_a_cli_leaves_an_unsupported_provider_untouched() {
    let mut env = HashMap::new();
    let mut args = Vec::new();
    let attached = attach_cli(
        crate::protocol::HarnessProvider::Codex,
        "pty-2",
        &mut env,
        &mut args,
    );
    assert_eq!(attached, None);
    assert!(
        args.is_empty(),
        "no flag we have not verified is guessed at"
    );
    assert!(env.is_empty(), "and no environment is written either");
}

#[test]
fn attaching_a_cli_passes_the_operators_tool_mode_through() {
    let mut env = HashMap::from([(super::super::TOOL_MODE_ENV.to_string(), "run".to_string())]);
    let mut args = Vec::new();
    attach_cli(
        crate::protocol::HarnessProvider::Claude,
        "pty-3",
        &mut env,
        &mut args,
    );
    let document: Value = serde_json::from_str(&args[1]).expect("a JSON document");
    assert_eq!(
        document["mcpServers"]["medulla"]["env"][super::super::TOOL_MODE_ENV],
        "run"
    );
}

#[test]
fn revoking_without_a_control_plane_is_a_no_op_rather_than_a_panic() {
    revoke_session("a session this process never granted");
}

/// The file [`ServerSpec::write_config_file`] writes is the one safe place for
/// the bearer grant: not on argv (world-readable via `/proc/<pid>/cmdline`),
/// and not the harness's own environment (inherited by every subprocess it
/// spawns, not just the one this file configures).
#[test]
fn write_config_file_carries_the_secret_the_argv_document_withholds() {
    let spec = server_spec(
        Some("run"),
        Some((PathBuf::from("/run/medulla.sock"), "s3cret".to_string())),
        true,
    )
    .expect("a server is served here");
    let session = "attach-test-carries-secret";

    let path = spec
        .write_config_file(session)
        .expect("the config file is writable");
    let contents = std::fs::read_to_string(&path).expect("the file was written");
    std::fs::remove_file(&path).ok();

    let document: Value = serde_json::from_str(&contents).expect("a JSON document");
    let env = &document["mcpServers"]["medulla"]["env"];
    assert_eq!(env[crate::control_socket::MCP_GRANT_ENV], "s3cret");
    assert_eq!(
        env[crate::control_socket::MCP_SOCKET_ENV],
        "/run/medulla.sock"
    );
    // The tool mode travels the same way non-secret env always has.
    assert_eq!(env[super::super::TOOL_MODE_ENV], "run");
}

/// Only this session's own user may read the file its grant went into — the
/// same property `/proc/<pid>/environ` has and `/proc/<pid>/cmdline` does not.
#[cfg(unix)]
#[test]
fn write_config_file_is_created_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let spec = server_spec(
        None,
        Some((PathBuf::from("/run/medulla.sock"), "s3cret".to_string())),
        false,
    )
    .expect("a fleet grant is served here");
    let session = "attach-test-owner-only";

    let path = spec
        .write_config_file(session)
        .expect("the config file is writable");
    let mode = std::fs::metadata(&path)
        .expect("the file exists")
        .permissions()
        .mode()
        & 0o777;
    std::fs::remove_file(&path).ok();

    assert_eq!(
        mode, 0o600,
        "the grant's file must not be group- or world-readable"
    );
}

#[test]
fn revoke_session_removes_the_config_file_it_named() {
    let spec = server_spec(
        None,
        Some((PathBuf::from("/run/medulla.sock"), "s3cret".to_string())),
        false,
    )
    .expect("a fleet grant is served here");
    let session = "attach-test-revoke-removes-file";
    let path = spec
        .write_config_file(session)
        .expect("the config file is writable");
    assert!(path.exists());

    revoke_session(session);

    assert!(
        !path.exists(),
        "revoking a session must remove any file its grant was minted into"
    );
}

/// The end-to-end property `an_operator_started_claude_is_handed_medullas_own_tools`
/// (`src/tui/src/ui/harness_pane/tests/session.rs`) cannot exercise on its own,
/// because that test never installs a control plane: with a real grant,
/// `attach_cli` must register through a file rather than the inline document,
/// and must never touch the process environment it was handed.
#[test]
fn attach_cli_never_writes_the_process_environment() {
    let mut env = HashMap::new();
    let mut args = Vec::new();
    attach_cli(
        crate::protocol::HarnessProvider::Claude,
        "attach-test-no-env-write",
        &mut env,
        &mut args,
    );
    assert!(
        env.is_empty(),
        "attach_cli must never write the harness's own environment: {env:?}"
    );
}
