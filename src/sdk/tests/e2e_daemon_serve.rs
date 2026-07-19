//! (Unix-only: relies on `/bin/echo` as a stand-in provider CLI and a Unix-friendly
//! temp layout.)
#![cfg(unix)]

//! End-to-end coverage for the `medulla daemon` CLI entry point
//! ([`medulla::daemon::run_daemon`]) driven in `--once` probe mode against a MOCK
//! tiny.place Signal server ([`mock_signal_server`]). No network, no real
//! provider CLI: `/bin/echo` stands in as a detectable provider binary, and the
//! daemon onboards + serves one drain cycle, then exits.
//!
//! `run_daemon` reads the *process* environment (it is the real CLI seam), so
//! these tests serialize on a process-wide lock while they mutate env. Each lives
//! in this dedicated integration binary so the mutation never leaks into the rest
//! of the suite.

mod support;

#[path = "support/mock_signal_server.rs"]
mod mock_signal_server;

use tokio::sync::{Mutex, MutexGuard};

use medulla::daemon::run_daemon;
use medulla::tinyplace::tinyplace::{LocalSigner, Signer};

use mock_signal_server::MockSignalServer;

/// Serializes process-env mutation across the tests in this binary. Async-aware
/// because every holder crosses awaits while the daemon runs.
static ENV_LOCK: Mutex<()> = Mutex::const_new(());

/// A temp dir removed on drop.
struct TempDir {
    path: std::path::PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "medulla-daemon-serve-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        TempDir { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn seed_hex(signer: &LocalSigner) -> String {
    signer.seed().iter().map(|b| format!("{b:02x}")).collect()
}

/// Env keys these tests touch, so we can clear/restore a clean slate under lock.
const TOUCHED: &[&str] = &[
    "MEDULLA_HOME",
    "MEDULLA_DEV",
    "TINYPLACE_CONFIG",
    "TINYPLACE_ENDPOINT",
    "TINYPLACE_SECRET_KEY",
    "TINYPLACE_CLAUDE_BIN",
    "TINYPLACE_CODEX_BIN",
    "TINYPLACE_OPENCODE_BIN",
    "TINYPLACE_OPENHUMAN_OWNER",
    "TINYPLACE_HARNESS_DM_TO",
    "OPENHUMAN_OWNER_AGENT",
];

fn clear_touched() {
    for key in TOUCHED {
        std::env::remove_var(key);
    }
}

/// Set a clean identity + endpoint env for a daemon run and return the guard that
/// keeps the process-env mutation exclusive.
async fn lock_env() -> MutexGuard<'static, ()> {
    let guard = ENV_LOCK.lock().await;
    clear_touched();
    guard
}

#[tokio::test]
async fn once_mode_serves_a_drain_cycle_and_exits() {
    let _guard = lock_env().await;
    let server = MockSignalServer::start().await;
    let home = TempDir::new("home");
    let cfg = TempDir::new("cfg");
    let signer = LocalSigner::generate();

    std::env::set_var("MEDULLA_HOME", &home.path);
    std::env::set_var(
        "TINYPLACE_CONFIG",
        cfg.path.join("config.json").to_str().unwrap(),
    );
    std::env::set_var("TINYPLACE_ENDPOINT", &server.base_url);
    std::env::set_var("TINYPLACE_SECRET_KEY", seed_hex(&signer));
    // `/bin/echo` is an executable on PATH → codex is "detected".
    std::env::set_var("TINYPLACE_CODEX_BIN", "/bin/echo");

    let args: Vec<String> = [
        "--once",
        "--no-onboard",
        "--providers",
        "codex",
        "--poll-ms",
        "20",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    run_daemon(&args, None)
        .await
        .expect("--once run should succeed");

    // The daemon wrote a worker profile during headless onboarding.
    assert!(home.path.join("worker.json").exists());
    clear_touched();
}

#[tokio::test]
async fn once_mode_onboards_handle_and_owner() {
    let _guard = lock_env().await;
    let server = MockSignalServer::start().await;
    let home = TempDir::new("home-onb");
    let cfg = TempDir::new("cfg-onb");
    let signer = LocalSigner::generate();
    let owner = LocalSigner::generate();

    std::env::set_var("MEDULLA_HOME", &home.path);
    std::env::set_var(
        "TINYPLACE_CONFIG",
        cfg.path.join("config.json").to_str().unwrap(),
    );
    std::env::set_var("TINYPLACE_ENDPOINT", &server.base_url);
    std::env::set_var("TINYPLACE_SECRET_KEY", seed_hex(&signer));
    std::env::set_var("TINYPLACE_CODEX_BIN", "/bin/echo");
    // An owner triggers the introduction DM + directory onboarding path.
    std::env::set_var("TINYPLACE_OPENHUMAN_OWNER", owner.agent_id());

    let args: Vec<String> = [
        "--once",
        "--providers",
        "codex",
        "--poll-ms",
        "20",
        "--handle",
        "medulla-test-bot",
        "--name",
        "Test Bot",
        "--skills",
        "rust,testing",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    run_daemon(&args, None)
        .await
        .expect("--once with onboarding should succeed");

    // Onboarding wrote the worker profile bound to the configured owner.
    let profile = std::fs::read_to_string(home.path.join("worker.json")).unwrap();
    assert!(profile.contains(&owner.agent_id()), "profile: {profile}");
    let _ = &server;
    clear_touched();
}

#[tokio::test]
async fn unknown_provider_flag_is_rejected() {
    let _guard = lock_env().await;
    let args: Vec<String> = ["--providers", "not-a-provider"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let err = run_daemon(&args, None).await.unwrap_err();
    assert!(err.to_string().contains("unknown provider"), "got: {err}");
    clear_touched();
}

#[tokio::test]
async fn no_detected_providers_bails() {
    let _guard = lock_env().await;
    // Point every provider bin at a nonexistent path so none are detected.
    std::env::set_var("TINYPLACE_CLAUDE_BIN", "/no/such/claude");
    std::env::set_var("TINYPLACE_CODEX_BIN", "/no/such/codex");
    std::env::set_var("TINYPLACE_OPENCODE_BIN", "/no/such/opencode");
    let err = run_daemon(&[], None).await.unwrap_err();
    assert!(
        err.to_string().contains("no coding-agent CLI found"),
        "got: {err}"
    );
    clear_touched();
}

#[tokio::test]
async fn default_provider_must_be_detected() {
    let _guard = lock_env().await;
    // Only codex is available, but the operator asks for claude as default.
    std::env::set_var("TINYPLACE_CODEX_BIN", "/bin/echo");
    let args: Vec<String> = ["--providers", "codex", "--default-provider", "claude"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let err = run_daemon(&args, None).await.unwrap_err();
    assert!(err.to_string().contains("is not available"), "got: {err}");
    clear_touched();
}
