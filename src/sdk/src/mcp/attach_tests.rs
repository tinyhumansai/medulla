//! Unit tests for the shared harness attachment policy.
//!
//! The control plane is a process-wide `OnceLock`, so these tests never install
//! one: they cover the decisions that hold with or without a fleet, and the
//! grant-carrying paths are exercised end to end in
//! `src/sdk/tests/feature_mcp_fleet.rs` where a real socket is bound.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::Value;

use super::*;

/// A spec with no fleet grant, as a host with workflows on and no plane builds.
fn spec_without_fleet() -> ServerSpec {
    server_spec(None, None, true).expect("workflows on is enough to serve a server")
}

/// Serializes the tests below that write a real `--mcp-config` file: those
/// resolve their directory through `mcp_state_dir`/`process_env`, which reads
/// this *process's* real environment rather than a per-test `HashMap` — unlike
/// every other `medulla_home`-dependent test in this crate, which injects its
/// own map and needs no such guard. Two of these tests racing on the same
/// `MEDULLA_HOME` mutation would each think it owned the value.
static HOME_ENV_LOCK: Mutex<()> = Mutex::new(());

/// Points `MEDULLA_HOME` at a scratch directory for the life of one test, so
/// `write_config_file`/`revoke_session`/`sweep_stale_config_files` never touch
/// the real developer or CI account while these tests run. Restores whatever
/// was there before on drop.
struct ScratchHome {
    _guard: std::sync::MutexGuard<'static, ()>,
    _dir: tempfile::TempDir,
    previous: Option<String>,
}

impl ScratchHome {
    fn install() -> Self {
        let guard = HOME_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().expect("a scratch home");
        let previous = std::env::var("MEDULLA_HOME").ok();
        std::env::set_var("MEDULLA_HOME", dir.path());
        Self {
            _guard: guard,
            _dir: dir,
            previous,
        }
    }
}

impl Drop for ScratchHome {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("MEDULLA_HOME", value),
            None => std::env::remove_var("MEDULLA_HOME"),
        }
    }
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
        "claude",
        "pty-1",
        &mut env,
        &mut args,
        None,
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
        "codex",
        "pty-2",
        &mut env,
        &mut args,
        None,
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
        "claude",
        "pty-3",
        &mut env,
        &mut args,
        None,
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
    let _home = ScratchHome::install();
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

    let _home = ScratchHome::install();
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

/// The property that actually closes the class of attack the module docs
/// describe: a pre-existing entry at the target path — a symlink included —
/// must never be followed and written through. `create_new` is what
/// guarantees this; a plain `create` would happily write the secret to
/// wherever `elsewhere` points.
#[cfg(unix)]
#[test]
fn write_owner_only_refuses_a_pre_existing_path_rather_than_following_it() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let target = dir.path().join("planted");
    let elsewhere = dir.path().join("elsewhere");
    std::os::unix::fs::symlink(&elsewhere, &target).expect("a symlink at the target path");

    let result = write_owner_only(&target, "{ \"secret\": true }");

    assert!(
        result.is_err(),
        "an existing entry at the target path must make the write fail, not succeed through it"
    );
    assert!(
        !elsewhere.exists(),
        "the secret must never land at wherever a pre-planted symlink points"
    );
}

#[test]
fn revoke_session_removes_the_config_file_it_named() {
    let _home = ScratchHome::install();
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

// `sweep_stale_config_files` is deliberately not unit-tested here. It is keyed
// off the socket the caller passes, while `write_config_file` keys off the
// *installed* plane — the two agree in production, where the sweep is handed
// the socket that plane is about to be published with, and cannot agree in a
// unit test, which must never install a plane (see this module's docs).
// `src/sdk/tests/feature_mcp_sweep_scope.rs` covers it with a real plane
// installed, and asserts the stronger property besides: a sibling instance's
// files survive this instance's sweep.

/// `write_config_file` is public API, and the internal caller's session keys
/// are UUID-derived, but nothing stops an external one from handing it a `/`
/// or a `..` — which `PathBuf::join` would otherwise turn into a path outside
/// [`mcp_state_dir`] entirely (an absolute joinee even *replaces* the base).
#[test]
fn write_config_file_refuses_a_session_that_is_not_a_plain_filename() {
    let _home = ScratchHome::install();
    let spec = server_spec(
        None,
        Some((PathBuf::from("/run/medulla.sock"), "s3cret".to_string())),
        false,
    )
    .expect("a fleet grant is served here");

    for unsafe_session in [
        "../escaped",
        "nested/path",
        "/etc/cron.d/evil",
        "..",
        ".",
        "",
        "back\\slash",
        // Drive-relative on Windows: no separator at all, but `join` treats it
        // as replacing the base, so a denylist of separators let it through.
        "C:temp",
        "C:/absolute",
        // A bare drive, and the UNC-ish shape, for the same reason.
        "C:",
        "\\\\server\\share",
    ] {
        let result = spec.write_config_file(unsafe_session);
        assert!(
            result.is_err(),
            "{unsafe_session:?} must be refused, not turned into a path"
        );
    }
}

/// The same refusal on the revoke side: a caller-supplied session that fails
/// validation must not make `revoke_session` guess a path and delete
/// whatever it happens to name.
///
/// Reproduces the exact escape a naive `mcp_state_dir().join(format!("{session}.json"))`
/// would take: `session` set to an *absolute* path with no extension, so the
/// `.json` this always appends lands on a real file's exact name, and
/// `PathBuf::join`'s own rule for an absolute argument — it replaces the base
/// outright rather than nesting under it — puts the "escaped" path exactly on
/// that file.
#[test]
fn revoke_session_refuses_to_delete_for_an_unsafe_session_key() {
    let _home = ScratchHome::install();
    let outside = tempfile::tempdir().expect("a directory outside any mcp directory");
    let sentinel_path = outside.path().join("important.json");
    std::fs::write(&sentinel_path, "do not delete me").expect("the sentinel file is writable");
    let session_without_extension = outside.path().join("important");

    revoke_session(&session_without_extension.display().to_string());

    assert!(
        sentinel_path.exists(),
        "an unrelated file named by an unsafe (absolute) session key must survive revoke_session"
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
        "claude",
        "attach-test-no-env-write",
        &mut env,
        &mut args,
        None,
    );
    assert!(
        env.is_empty(),
        "attach_cli must never write the harness's own environment: {env:?}"
    );
}

#[test]
fn attaching_a_cli_tells_the_server_which_session_it_serves() {
    // Without this the tool server cannot attribute a run it starts, and the
    // Agents rail has nothing to nest the run under.
    let mut env = HashMap::new();
    let mut args = Vec::new();
    attach_cli(
        crate::protocol::HarnessProvider::Claude,
        "claude",
        "pty-origin",
        &mut env,
        &mut args,
        None,
    );
    let document: Value = serde_json::from_str(&args[1]).expect("a JSON document");
    assert_eq!(
        document["mcpServers"]["medulla"]["env"][super::ORIGIN_SESSION_ENV],
        "pty-origin"
    );
    assert!(
        !env.contains_key(super::ORIGIN_SESSION_ENV),
        "it belongs to the server's own spawn, not to the harness's environment"
    );
}

#[test]
fn a_blank_session_key_stamps_nothing() {
    // A caller with no session to name must not write an empty attribution the
    // rail would then try to match runs against.
    let spec = ServerSpec {
        name: "medulla",
        command: "medulla".into(),
        args: vec!["mcp".into()],
        env: Vec::new(),
        secret_env: Vec::new(),
    }
    .for_session("   ");
    assert!(spec.env.is_empty());
}

/// The file transport carries the session too, so the ACP door attributes runs
/// exactly as the argv door does.
#[test]
fn a_written_config_file_carries_the_session_alongside_the_grant() {
    let _home = ScratchHome::install();
    let session = "attach-test-carries-session";
    let spec = server_spec(
        Some("run"),
        Some((PathBuf::from("/run/medulla.sock"), "s3cret".to_string())),
        true,
    )
    .expect("a server is served here")
    .for_session(session);

    let path = spec
        .write_config_file(session)
        .expect("the config file is writable");
    let contents = std::fs::read_to_string(&path).expect("the file was written");
    std::fs::remove_file(&path).ok();

    let document: Value = serde_json::from_str(&contents).expect("a JSON document");
    let env = &document["mcpServers"]["medulla"]["env"];
    assert_eq!(env[super::ORIGIN_SESSION_ENV], session);
    assert_eq!(
        env[crate::control_socket::MCP_GRANT_ENV],
        "s3cret",
        "and the grant is still there beside it"
    );
}
