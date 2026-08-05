#![cfg(unix)]

//! `attach_cli` withholding the fleet grant from an overridden provider
//! binary — the fix for the P1 Codex found on `pty-mcp-attach-security-fixup`
//! (medulla#177): a provider-binary override runs as the very process
//! `--mcp-config` registers this onto, so it receives that registration's
//! argv itself and can open the file it names directly, no matter how the
//! secret half is delivered.
//!
//! A real `ActiveControlPlane` has to be installed to prove this — the plane
//! is a process-wide `OnceLock` (see `mcp::attach`'s own unit tests, which
//! deliberately never install one), so this lives in its own test binary
//! rather than the crate's unit tests, where installing one would leak into
//! every other unit test that runs in the same process.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Once};

use medulla::control_socket::{ActiveControlPlane, GrantRegistry};
use medulla::mcp::attach_cli;
use medulla::protocol::HarnessProvider;

/// Serialises the tests in this binary against the one thing they all mutate.
///
/// `MEDULLA_HOME` is process-global and the test harness runs these four
/// concurrently, so without this one test can retarget another's home
/// mid-flight — or drop the scratch directory underneath it — and the
/// default-provider case then intermittently observes a failed config write
/// and no grant.
static HOME: Mutex<()> = Mutex::new(());

/// A scratch `MEDULLA_HOME`, held for the whole of one test.
///
/// `write_config_file`/`revoke_session` resolve the home from this process's
/// real environment (see `mcp::attach`'s module docs), so it has to be set
/// rather than passed. Dropping this restores whatever was there before and
/// releases the lock, in that order, so the next test sets its own home
/// against a clean slate.
struct ScratchHome {
    // Never read: it exists to keep the directory alive for the test's
    // lifetime and delete it on drop.
    _dir: tempfile::TempDir,
    previous: Option<std::ffi::OsString>,
    // Declared last so it is dropped last: the guard must outlive the restore
    // above it, or the next test could observe this one's value.
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl Drop for ScratchHome {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("MEDULLA_HOME", value),
            None => std::env::remove_var("MEDULLA_HOME"),
        }
    }
}

fn scratch_home() -> ScratchHome {
    // A poisoned lock means some earlier test panicked while holding it. That
    // is a failure already reported; taking the guard anyway keeps this test
    // from failing for someone else's reason.
    let lock = HOME.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var_os("MEDULLA_HOME");
    let dir = tempfile::tempdir().expect("a scratch home");
    std::env::set_var("MEDULLA_HOME", dir.path());
    ScratchHome {
        _dir: dir,
        previous,
        _lock: lock,
    }
}

/// Install a control plane exactly once for this process: `install` is a
/// `OnceLock` and refuses a second call, and the tests below do not depend on
/// which of them happened to install it — both only need *some* active plane.
static INSTALL: Once = Once::new();

fn ensure_control_plane() {
    INSTALL.call_once(|| {
        medulla::control_socket::install(ActiveControlPlane {
            socket: PathBuf::from("/run/medulla-test.sock"),
            grants: GrantRegistry::new(),
            max_depth: 2,
            max_in_flight: 4,
        });
    });
}

/// The environment here is deliberately *empty* while the binary is
/// overridden. That is the regression: `PtySessionExecutor` selects its
/// executable from `self.env` and hands the child a separately derived
/// environment, so a decision that re-derived the binary from the environment
/// it was passed would see no override and hand a wrapper the live grant
/// (medulla#177). The binary is the only thing asked about.
#[test]
fn an_overridden_provider_binary_gets_no_fleet_grant() {
    let _home = scratch_home();
    ensure_control_plane();

    let mut env = HashMap::new();
    let mut args = Vec::new();

    let granted = attach_cli(
        HarnessProvider::Claude,
        "/opt/untrusted/claude",
        "override-test-session",
        &mut env,
        &mut args,
        None,
    );

    assert_eq!(
        granted, None,
        "an overridden binary must never be handed a fleet grant to redeem"
    );
    let flag = args
        .iter()
        .position(|arg| arg == "--mcp-config")
        .expect("workflow tools are still registered");
    let document = &args[flag + 1];
    // Withheld means never minted, so the registration takes the inline,
    // no-file branch — the same one a host with no control plane at all
    // takes — rather than `write_config_file` naming a path on argv.
    let parsed: serde_json::Value =
        serde_json::from_str(document).expect("no grant means no file, so this is inline JSON");
    assert!(
        parsed["mcpServers"]["medulla"]
            .get("env")
            .and_then(|env| env.get(medulla::control_socket::MCP_SOCKET_ENV))
            .is_none(),
        "the control socket must never reach an overridden binary's registration: {document}"
    );
}

#[test]
fn the_default_provider_binary_still_gets_its_fleet_grant() {
    let _home = scratch_home();
    ensure_control_plane();

    let mut env = HashMap::new();
    let mut args = Vec::new();

    let granted = attach_cli(
        HarnessProvider::Claude,
        "claude",
        "default-bin-test-session",
        &mut env,
        &mut args,
        None,
    );

    assert!(
        granted.is_some(),
        "the verified default binary must still receive its fleet grant"
    );
    let flag = args
        .iter()
        .position(|arg| arg == "--mcp-config")
        .expect("claude is registered through --mcp-config");
    let path = &args[flag + 1];
    assert!(
        serde_json::from_str::<serde_json::Value>(path).is_err(),
        "a real grant must register through write_config_file's path, not inline JSON: {path}"
    );
    medulla::mcp::revoke_session(granted.as_deref().unwrap());
}

/// An operator running a legitimate wrapper binary loses `fleet_*` silently
/// unless something says why. `attach_cli` is told about the log seam and
/// must use it, exactly when withholding the grant actually cost this launch
/// one — a control plane is bound here, so it would otherwise have gotten it.
#[test]
fn an_overridden_binary_with_a_bound_plane_logs_why_the_grant_was_withheld() {
    let _home = scratch_home();
    ensure_control_plane();

    let mut env = HashMap::new();
    let mut args = Vec::new();
    let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = lines.clone();
    let log: medulla::daemon::LogFn = Arc::new(move |line: &str| {
        sink.lock().unwrap().push(line.to_string());
    });

    let granted = attach_cli(
        HarnessProvider::Claude,
        "/opt/wrapper/claude",
        "override-log-test-session",
        &mut env,
        &mut args,
        Some(&log),
    );

    assert_eq!(granted, None);
    let lines = lines.lock().unwrap();
    assert_eq!(lines.len(), 1, "exactly one note per launch: {lines:?}");
    assert!(
        lines[0].contains("/opt/wrapper/claude") && lines[0].contains("withheld"),
        "the note must name the overriding binary and say the grant was \
         withheld, so an operator knows what to undo: {}",
        lines[0]
    );
}

/// The redundant case from `bin_is_overridden_only_when_the_resolved_binary_actually_differs`
/// (`protocol::env`'s own unit tests) carried through end to end: naming the
/// default binary's own name changes nothing, so nothing is withheld and
/// nothing is logged.
#[test]
fn naming_the_default_binary_via_the_override_key_logs_nothing() {
    let _home = scratch_home();
    ensure_control_plane();

    let mut env = HashMap::new();
    let mut args = Vec::new();
    let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = lines.clone();
    let log: medulla::daemon::LogFn = Arc::new(move |line: &str| {
        sink.lock().unwrap().push(line.to_string());
    });

    let granted = attach_cli(
        HarnessProvider::Claude,
        // What `provider_bin` resolves a redundant `TINYVERSE_CLAUDE_BIN=claude`
        // to: the default's own name, which is not an override.
        "claude",
        "redundant-override-test-session",
        &mut env,
        &mut args,
        Some(&log),
    );

    assert!(
        granted.is_some(),
        "naming the default binary's own name must not count as an override"
    );
    assert!(
        lines.lock().unwrap().is_empty(),
        "nothing was withheld, so nothing should be logged"
    );
    medulla::mcp::revoke_session(granted.as_deref().unwrap());
}

/// The P1 Codex found on this branch: a hook shim needs a real grant to
/// report with, and — unlike the fleet grant — withholding it from an
/// overridden binary would leave every hook Medulla installs unable to
/// report, since the override withholding exists to protect fleet
/// capability that a hook-only grant never carries. See
/// `medulla::mcp::attach::local_hook_grant`.
#[test]
fn a_hook_only_grant_is_minted_even_for_an_overridden_binary() {
    let _home = scratch_home();
    ensure_control_plane();

    // `attach_cli` withholds the fleet grant here — proven above by
    // `an_overridden_provider_binary_gets_no_fleet_grant` — but the hook-only
    // grant this test mints is independent of that decision.
    let (socket, token) = medulla::mcp::local_hook_grant("hook-only-override-test-session").expect(
        "a hook-only grant must be minted whenever a control plane is bound, \
             regardless of any provider-binary override",
    );
    assert_eq!(socket, PathBuf::from("/run/medulla-test.sock"));
    assert!(!token.is_empty());
    medulla::mcp::revoke_session("hook-only-override-test-session");
}
