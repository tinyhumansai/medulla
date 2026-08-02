//! Tests for the `medulla:shell` native tool.
//!
//! Split out of [`super::tests`] (see that module's doc comment) once the
//! scripting cases pushed it over the repository's 500-line file ceiling.
//! Everything here exercises [`super::caps::tools::MedullaToolInvoker`]
//! dispatching to `medulla:shell`, which is the tool surface a `code` node and
//! an authored workflow step both go through.
//!
//! `medulla:shell` defaults to `ScriptLanguage::Shell` when a case gives no
//! `language`, so any case that actually runs a script under that default is
//! `#[cfg(unix)]` — Windows refuses that language rather than emulating a
//! POSIX shell for it (see `flow_engine::caps::script::run_script`). Cases
//! that fail before dispatching a script (the `allowCode` gate, an unknown
//! language name) are platform-independent and stay ungated.

use std::sync::Arc;

use serde_json::json;
use tinyflows::caps::ToolInvoker;

use super::caps::tools::MedullaToolInvoker;
use super::settings::CapabilitySettings;

/// Settings with script execution turned on, rooted at `workspace`.
fn scripting_settings(workspace: &std::path::Path) -> Arc<CapabilitySettings> {
    let mut settings = CapabilitySettings::rooted_at(workspace);
    settings.allow_code = true;
    settings.workspace = workspace.to_string_lossy().to_string();
    Arc::new(settings)
}

#[tokio::test]
async fn the_shell_tool_honors_an_explicit_operator_opt_out() {
    let root = tempfile::tempdir().unwrap();
    let mut denied = CapabilitySettings::rooted_at(root.path());
    denied.allow_code = false;

    let err = MedullaToolInvoker::new(Arc::new(denied))
        .invoke("medulla:shell", json!({ "script": "echo hi" }), None)
        .await
        .expect_err("explicitly disabled");

    // The same decision `code` nodes are gated on, and the message has to name
    // the switch rather than just refusing.
    assert!(err.to_string().contains("allowCode"), "got {err}");
}

// Unix-only: `medulla:shell` defaults to `ScriptLanguage::Shell`, which
// `run_script` refuses on Windows rather than emulating a POSIX shell
// there (see the `#[cfg(windows)]` guard in `flow_engine::caps::script`).
#[cfg(unix)]
#[tokio::test]
async fn the_shell_tool_runs_in_the_workspace_so_a_step_can_touch_the_project() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("VERSION"), "1.2.3").unwrap();

    let result = MedullaToolInvoker::new(scripting_settings(root.path()))
        .invoke("medulla:shell", json!({ "script": "cat VERSION" }), None)
        .await
        .expect("runs");

    // This is the whole difference from a `code` node: a step that means to
    // read the repository has to actually be in it.
    assert_eq!(result["output"], json!("1.2.3"));
}

// Unix-only: `medulla:shell` defaults to `ScriptLanguage::Shell`, which
// `run_script` refuses on Windows rather than emulating a POSIX shell
// there (see the `#[cfg(windows)]` guard in `flow_engine::caps::script`).
#[cfg(unix)]
#[tokio::test]
async fn the_shell_tool_hands_its_input_to_the_script_and_returns_structured_output() {
    let root = tempfile::tempdir().unwrap();

    let result = MedullaToolInvoker::new(scripting_settings(root.path()))
        .invoke(
            "medulla:shell",
            json!({ "script": "cat", "input": { "issues": 3 } }),
            None,
        )
        .await
        .expect("runs");

    assert_eq!(result["output"], json!({ "issues": 3 }));
}

// Unix-only: `medulla:shell` defaults to `ScriptLanguage::Shell`, which
// `run_script` refuses on Windows rather than emulating a POSIX shell
// there (see the `#[cfg(windows)]` guard in `flow_engine::caps::script`).
#[cfg(unix)]
#[tokio::test]
async fn the_shell_tool_keeps_stderr_from_a_script_that_succeeded() {
    let root = tempfile::tempdir().unwrap();

    let result = MedullaToolInvoker::new(scripting_settings(root.path()))
        .invoke(
            "medulla:shell",
            json!({ "script": "echo skipped 2 files >&2; echo ok" }),
            None,
        )
        .await
        .expect("runs");

    assert_eq!(result["output"], json!("ok"));
    assert_eq!(result["stderr"], json!("skipped 2 files"));
}

#[tokio::test]
async fn the_shell_tool_can_run_another_language_in_the_workspace() {
    let root = tempfile::tempdir().unwrap();
    if std::process::Command::new("python3")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .status()
        .is_err()
    {
        return;
    }

    let result = MedullaToolInvoker::new(scripting_settings(root.path()))
        .invoke(
            "medulla:shell",
            json!({ "language": "python", "script": "print('from python')" }),
            None,
        )
        .await
        .expect("runs");

    assert_eq!(result["output"], json!("from python"));
}

#[tokio::test]
async fn the_shell_tool_says_what_is_missing_rather_than_running_an_empty_script() {
    let root = tempfile::tempdir().unwrap();
    let invoker = MedullaToolInvoker::new(scripting_settings(root.path()));

    let missing = invoker
        .invoke("medulla:shell", json!({}), None)
        .await
        .expect_err("no script");
    assert!(missing.to_string().contains("args.script"), "{missing}");

    let unknown = invoker
        .invoke(
            "medulla:shell",
            json!({ "language": "ruby", "script": "puts 1" }),
            None,
        )
        .await
        .expect_err("no such language");
    // Refused rather than guessed: running the wrong interpreter on someone's
    // script is worse than saying the name was not recognised.
    assert!(unknown.to_string().contains("javascript"), "{unknown}");
}

// Unix-only: `medulla:shell` defaults to `ScriptLanguage::Shell`, which
// `run_script` refuses on Windows rather than emulating a POSIX shell
// there (see the `#[cfg(windows)]` guard in `flow_engine::caps::script`).
#[cfg(unix)]
#[tokio::test]
async fn a_failing_shell_step_fails_the_node_with_the_scripts_own_error() {
    let root = tempfile::tempdir().unwrap();

    let err = MedullaToolInvoker::new(scripting_settings(root.path()))
        .invoke(
            "medulla:shell",
            json!({ "script": "echo no such target >&2; exit 1" }),
            None,
        )
        .await
        .expect_err("fails");

    assert!(err.to_string().contains("no such target"), "{err}");
}
