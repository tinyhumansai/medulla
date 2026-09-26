//! End-to-end coverage for the `medulla run` command line (`src/run.rs`).
//!
//! `run` used to attach an external `medulla-serve` unix socket, and this suite
//! drove the installed binary against an in-test NDJSON stub of it. The core is
//! embedded in the process now, so there is no socket to stub and no attach
//! handshake to assert — what is left to cover here is the command-line
//! contract, which stays fast and offline.
//!
//! Driving a real turn is deliberately *not* covered: booting the core is cheap
//! but producing a reply needs a live model, which would make this suite
//! non-deterministic and network-dependent.

use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

/// Run the workspace binary with an isolated home and no inherited credentials
/// or model keys, mirroring the `e2e_cli` harness.
fn run(args: &[&str], home: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_medulla"))
        .args(args)
        .current_dir(home)
        .env("MEDULLA_HOME", home)
        .env_remove("MEDULLA_TOKEN")
        .env_remove("MEDULLA_CORE_SOCKET")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("MEDULLA_BACKEND_URL")
        .output()
        .expect("the medulla binary should run")
}

#[test]
fn run_without_an_instruction_is_a_usage_error() {
    let home = TempDir::new().unwrap();
    // No instruction text: the parser rejects it before booting anything, so
    // this stays fast and offline.
    let out = run(&["run"], home.path());
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("instruction"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn run_rejects_the_retired_core_socket_flag() {
    let home = TempDir::new().unwrap();
    // Loud, not silent. An unrecognized token joins the instruction, so without
    // an explicit rejection this would submit "--core-socket <path> reconcile"
    // to the agent as prompt text and look like it had worked.
    let out = run(
        &[
            "run",
            "--core-socket",
            "/nonexistent/serve.sock",
            "reconcile",
        ],
        home.path(),
    );
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--core-socket"), "stderr: {stderr}");
}
