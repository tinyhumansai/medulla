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

/// A path this platform actually considers absolute.
const fn absolute_path() -> &'static str {
    #[cfg(windows)]
    {
        r"C:\Windows\System32\drivers\etc\hosts"
    }
    #[cfg(not(windows))]
    {
        "/etc/profile"
    }
}

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

// Unix-only: `medulla:shell` defaults to `ScriptLanguage::Shell`, which
// `run_script` refuses on Windows rather than emulating a POSIX shell
// there (see the `#[cfg(windows)]` guard in `flow_engine::caps::script`).
#[cfg(unix)]
#[tokio::test]
async fn a_step_can_run_a_script_file_the_repository_already_has() {
    // The point of `script_path`: a project's own `scripts/release.sh` becomes
    // a workflow step without being pasted into the graph, where it would drift
    // from the copy the repository actually maintains.
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("scripts")).unwrap();
    std::fs::write(
        root.path().join("scripts/release.sh"),
        "echo shipped $(basename \"$PWD\" > /dev/null; echo v1)\n",
    )
    .unwrap();

    let result = MedullaToolInvoker::new(scripting_settings(root.path()))
        .invoke(
            "medulla:shell",
            json!({ "script_path": "scripts/release.sh" }),
            None,
        )
        .await
        .expect("runs");

    assert_eq!(result["output"], json!("shipped v1"));
}

// Unix-only: `medulla:shell` defaults to `ScriptLanguage::Shell`, which
// `run_script` refuses on Windows rather than emulating a POSIX shell
// there (see the `#[cfg(windows)]` guard in `flow_engine::caps::script`).
#[cfg(unix)]
#[tokio::test]
async fn a_step_can_narrow_the_directory_it_runs_in() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("crate-a")).unwrap();
    std::fs::write(root.path().join("crate-a/VERSION"), "2.0.0").unwrap();
    // The same name at the workspace root, so a step that ignored `cwd` would
    // read the wrong file and still look like it worked.
    std::fs::write(root.path().join("VERSION"), "0.0.0").unwrap();

    let result = MedullaToolInvoker::new(scripting_settings(root.path()))
        .invoke(
            "medulla:shell",
            json!({ "script": "cat VERSION", "cwd": "crate-a" }),
            None,
        )
        .await
        .expect("runs");

    assert_eq!(result["output"], json!("2.0.0"));
}

// Unix-only: `medulla:shell` defaults to `ScriptLanguage::Shell`, which
// `run_script` refuses on Windows rather than emulating a POSIX shell
// there (see the `#[cfg(windows)]` guard in `flow_engine::caps::script`).
#[cfg(unix)]
#[tokio::test]
async fn a_step_can_declare_the_environment_its_script_reads() {
    let root = tempfile::tempdir().unwrap();

    let result = MedullaToolInvoker::new(scripting_settings(root.path()))
        .invoke(
            "medulla:shell",
            json!({
                "script": "printf '%s/%s' \"$PROFILE\" \"$TARGET\"",
                "env": { "PROFILE": "release", "TARGET": "wasm" },
            }),
            None,
        )
        .await
        .expect("runs");

    assert_eq!(result["output"], json!("release/wasm"));
}

// Unix-only: `medulla:shell` defaults to `ScriptLanguage::Shell`, which
// `run_script` refuses on Windows rather than emulating a POSIX shell
// there (see the `#[cfg(windows)]` guard in `flow_engine::caps::script`).
#[cfg(unix)]
#[tokio::test]
async fn a_declared_variable_wins_over_the_inherited_one() {
    // `MEDULLA_INPUT` is set by the runner itself, so overriding it is the
    // sharpest available check that a declaration is layered last.
    let root = tempfile::tempdir().unwrap();

    let result = MedullaToolInvoker::new(scripting_settings(root.path()))
        .invoke(
            "medulla:shell",
            json!({
                "script": "printf '%s' \"$MEDULLA_INPUT\"",
                "env": { "MEDULLA_INPUT": "overridden" },
            }),
            None,
        )
        .await
        .expect("runs");

    assert_eq!(result["output"], json!("overridden"));
}

#[tokio::test]
async fn a_script_outside_the_workspace_is_refused_rather_than_run() {
    let root = tempfile::tempdir().unwrap();
    let invoker = MedullaToolInvoker::new(scripting_settings(root.path()));

    for (args, needle) in [
        (
            json!({ "script_path": "../../etc/profile" }),
            "must not traverse outside",
        ),
        (
            // Absolute on this platform specifically: `/etc/profile` has no
            // drive prefix, so Windows would take the traversal branch instead
            // and the assertion below would be checking the wrong message.
            json!({ "script_path": absolute_path() }),
            "must be relative to the workspace",
        ),
        (
            json!({ "script": "echo hi", "cwd": "../elsewhere" }),
            "must not traverse outside",
        ),
    ] {
        let err = invoker
            .invoke("medulla:shell", args.clone(), None)
            .await
            .expect_err("a path outside the workspace must be refused");
        assert!(err.to_string().contains(needle), "{args}: {err}");
    }
}

#[tokio::test]
async fn a_malformed_environment_is_refused_before_the_script_runs() {
    let root = tempfile::tempdir().unwrap();

    let err = MedullaToolInvoker::new(scripting_settings(root.path()))
        .invoke(
            "medulla:shell",
            json!({ "script": "echo hi", "env": { "PROFILE": 3 } }),
            None,
        )
        .await
        .expect_err("a non-string value must be refused");

    assert!(err.to_string().contains("must be a string"), "{err}");
}

#[tokio::test]
async fn a_step_names_one_script_or_the_other_but_never_both() {
    let root = tempfile::tempdir().unwrap();
    let invoker = MedullaToolInvoker::new(scripting_settings(root.path()));

    let both = invoker
        .invoke(
            "medulla:shell",
            json!({ "script": "echo hi", "script_path": "scripts/build.sh" }),
            None,
        )
        .await
        .expect_err("an ambiguous call must not run");
    assert!(both.to_string().contains("not both"), "{both}");

    // The "nothing at all" message has to name both ways in, not just one.
    let neither = invoker
        .invoke("medulla:shell", json!({ "input": 1 }), None)
        .await
        .expect_err("a call with no script must not run");
    assert!(
        neither.to_string().contains("args.script_path"),
        "{neither}"
    );
}

#[tokio::test]
async fn a_malformed_path_argument_is_refused_rather_than_ignored() {
    // Failing open here is the dangerous shape: a `cwd` quietly dropped runs
    // the step at the workspace root, reads a different file, and reports
    // success — a wrong answer that looks like a right one.
    let root = tempfile::tempdir().unwrap();
    let invoker = MedullaToolInvoker::new(scripting_settings(root.path()));

    for (args, needle) in [
        (
            json!({ "script": "cat VERSION", "cwd": 3 }),
            "`args.cwd` must be a non-empty path",
        ),
        (
            json!({ "script": "cat VERSION", "cwd": "  " }),
            "`args.cwd` must be a non-empty path",
        ),
        (
            json!({ "script": 7 }),
            "`args.script` must be a non-empty script",
        ),
        (
            json!({ "script_path": ["scripts/build.sh"] }),
            "`args.script_path` must be a non-empty path",
        ),
    ] {
        let err = invoker
            .invoke("medulla:shell", args.clone(), None)
            .await
            .expect_err("a malformed argument must be refused");
        assert!(err.to_string().contains(needle), "{args}: {err}");
    }
}

#[tokio::test]
async fn an_explicit_null_argument_reads_as_absent() {
    // `null` is how a templating layer spells "I had nothing for this", so it
    // means absent rather than malformed.
    let root = tempfile::tempdir().unwrap();

    let err = MedullaToolInvoker::new(scripting_settings(root.path()))
        .invoke(
            "medulla:shell",
            json!({ "script": null, "script_path": null }),
            None,
        )
        .await
        .expect_err("a call with no script must not run");

    assert!(err.to_string().contains("is required"), "{err}");
}

// Unix-only for the same reason as the cases above: these actually dispatch a
// shell script, which Windows refuses rather than emulating.
#[cfg(unix)]
#[tokio::test]
async fn a_step_may_name_its_own_interpreter() {
    let root = tempfile::tempdir().unwrap();

    let result = MedullaToolInvoker::new(scripting_settings(root.path()))
        .invoke(
            "medulla:shell",
            json!({ "script": "echo picked", "shell": "sh" }),
            None,
        )
        .await
        .expect("runs");

    assert_eq!(result["output"], json!("picked"));
}

#[cfg(unix)]
#[tokio::test]
async fn interpreter_arguments_alone_still_change_how_the_host_shell_runs() {
    // `shell_args` with no `shell` means "the host's shell, with these flags".
    // Dropping them would run the script plainly and look like the flag did
    // nothing — the failure this case exists to prevent.
    let root = tempfile::tempdir().unwrap();

    let result = MedullaToolInvoker::new(scripting_settings(root.path()))
        .invoke(
            "medulla:shell",
            json!({ "script": "echo traced", "shell_args": ["-x"] }),
            None,
        )
        .await
        .expect("runs");

    assert_eq!(result["output"], json!("traced"));
    assert!(
        result["stderr"]
            .as_str()
            .unwrap_or_default()
            .contains("echo traced"),
        "expected an -x trace, got {}",
        result["stderr"]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn the_host_shell_setting_decides_when_a_step_names_none() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = CapabilitySettings::rooted_at(root.path());
    settings.allow_code = true;
    settings.workspace = root.path().to_string_lossy().to_string();
    settings.shell = crate::flow_engine::caps::script::Interpreter::validated("sh", &["-x".into()])
        .expect("valid");

    let result = MedullaToolInvoker::new(Arc::new(settings))
        .invoke(
            "medulla:shell",
            json!({ "script": "echo configured" }),
            None,
        )
        .await
        .expect("runs");

    assert_eq!(result["output"], json!("configured"));
    assert!(
        result["stderr"]
            .as_str()
            .unwrap_or_default()
            .contains("echo configured"),
        "the operator's configured shell must be the one that runs it, got {}",
        result["stderr"]
    );
}

#[tokio::test]
async fn naming_an_interpreter_for_a_non_shell_language_is_refused() {
    // Quietly handing a `.py` file to a shell because the host configured one
    // is the worst kind of surprise, so the contradiction is named instead.
    let root = tempfile::tempdir().unwrap();

    let err = MedullaToolInvoker::new(scripting_settings(root.path()))
        .invoke(
            "medulla:shell",
            json!({ "script": "print(1)", "language": "python", "shell": "zsh" }),
            None,
        )
        .await
        .expect_err("contradictory");

    let message = err.to_string();
    assert!(message.contains("not \"python\""), "{message}");
    assert!(message.contains("args.shell"), "{message}");
}

#[tokio::test]
async fn malformed_interpreter_arguments_are_refused_rather_than_coerced() {
    let root = tempfile::tempdir().unwrap();
    let invoker = MedullaToolInvoker::new(scripting_settings(root.path()));

    let not_an_array = invoker
        .invoke(
            "medulla:shell",
            json!({ "script": "echo hi", "shell_args": "-l" }),
            None,
        )
        .await
        .expect_err("not an array");
    assert!(
        not_an_array.to_string().contains("array of strings"),
        "{not_an_array}"
    );

    let not_a_string = invoker
        .invoke(
            "medulla:shell",
            json!({ "script": "echo hi", "shell_args": [3] }),
            None,
        )
        .await
        .expect_err("not a string");
    assert!(
        not_a_string.to_string().contains("must be a string"),
        "{not_a_string}"
    );
}

#[tokio::test]
async fn a_relative_interpreter_path_is_refused_at_the_tool_surface() {
    let root = tempfile::tempdir().unwrap();

    let err = MedullaToolInvoker::new(scripting_settings(root.path()))
        .invoke(
            "medulla:shell",
            json!({ "script": "echo hi", "shell": "./sh" }),
            None,
        )
        .await
        .expect_err("relative");

    assert!(err.to_string().contains("relative path"), "{err}");
}

#[cfg(unix)]
#[tokio::test]
async fn a_step_spells_the_login_shell_the_same_way_the_config_does() {
    // `"user"` is a sentinel, not a program name — a step that writes it must
    // reach `$SHELL` rather than trying to spawn a binary called `user`, which
    // is the inconsistency between the two surfaces this pins.
    let root = tempfile::tempdir().unwrap();

    let result = MedullaToolInvoker::new(scripting_settings(root.path()))
        .invoke(
            "medulla:shell",
            json!({ "script": "echo \"${SHELL_KIND:-unset}\"", "shell": "user" }),
            None,
        )
        .await
        .expect("runs");

    // Whatever `$SHELL` names on the runner, the script ran under *a* shell
    // rather than failing to spawn `user`. Reaching output at all is the
    // assertion; naming the shell would pin the CI runner's login shell.
    assert!(result["output"].is_string(), "got {}", result["output"]);
}
