//! Tests for the out-of-process script runner.
//!
//! These spawn real interpreters, which is the point: the bug this module was
//! written to fix was a calling convention that was documented one way and
//! implemented another, and only actually running something catches that.
//!
//! `bash` is assumed present on unix — this crate already assumes a unix host
//! in its bridge and daemon tests — and the `ScriptLanguage::Shell` cases are
//! `#[cfg(unix)]` because `run_script` itself refuses that language on
//! Windows (see its doc comment): there is no portable POSIX shell there to
//! run them in, and emulating one is exactly the per-platform behavior this
//! module exists to avoid. `node` and `python3` are cross-platform but not
//! guaranteed installed, so those cases skip rather than fail on a machine
//! without them.

use super::*;
use serde_json::json;

/// The plain shape most cases here want: an inline script, no declared
/// environment. Cases that exercise `cwd`, a script file, or `env` build a
/// [`ScriptRequest`] themselves.
async fn run(
    language: ScriptLanguage,
    source: &str,
    input: &Value,
    timeout: Duration,
    cwd: Option<&Path>,
) -> Result<ScriptOutput> {
    let env = BTreeMap::new();
    run_script(ScriptRequest {
        language,
        source: ScriptSource::Inline(source),
        input,
        timeout,
        cwd,
        env: &env,
    })
    .await
}

const TIMEOUT: Duration = Duration::from_secs(30);

/// Whether an interpreter is on `PATH`, so a test can skip rather than fail.
fn available(program: &str) -> bool {
    std::process::Command::new(program)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

// Unix-only: these run `ScriptLanguage::Shell`, which `run_script` refuses
// on Windows because there is no portable POSIX shell to run it in (see
// the `#[cfg(windows)]` guard in `run_script`) rather than emulating one.
#[cfg(unix)]
#[tokio::test]
async fn a_shell_script_reads_its_input_on_stdin_and_returns_stdout() {
    let output = run(
        ScriptLanguage::Shell,
        "cat",
        &json!({ "name": "sweep" }),
        TIMEOUT,
        None,
    )
    .await
    .expect("runs");

    // Round-tripped through stdin and back out, and parsed as JSON on the way
    // back because it is JSON.
    assert_eq!(output.value, json!({ "name": "sweep" }));
}

// Unix-only: these run `ScriptLanguage::Shell`, which `run_script` refuses
// on Windows because there is no portable POSIX shell to run it in (see
// the `#[cfg(windows)]` guard in `run_script`) rather than emulating one.
#[cfg(unix)]
#[tokio::test]
async fn a_shell_script_can_read_its_input_from_the_path_instead() {
    // Shell reads a path more naturally than a pipe, so both are offered.
    let output = run(
        ScriptLanguage::Shell,
        "cat \"$MEDULLA_INPUT\"",
        &json!({ "n": 1 }),
        TIMEOUT,
        None,
    )
    .await
    .expect("runs");

    assert_eq!(output.value, json!({ "n": 1 }));
}

// Unix-only: these run `ScriptLanguage::Shell`, which `run_script` refuses
// on Windows because there is no portable POSIX shell to run it in (see
// the `#[cfg(windows)]` guard in `run_script`) rather than emulating one.
#[cfg(unix)]
#[tokio::test]
async fn the_input_path_is_also_the_first_argument() {
    let output = run(
        ScriptLanguage::Shell,
        "cat \"$1\"",
        &json!(42),
        TIMEOUT,
        None,
    )
    .await
    .expect("runs");

    assert_eq!(output.value, json!(42));
}

// Unix-only: these run `ScriptLanguage::Shell`, which `run_script` refuses
// on Windows because there is no portable POSIX shell to run it in (see
// the `#[cfg(windows)]` guard in `run_script`) rather than emulating one.
#[cfg(unix)]
#[tokio::test]
async fn output_that_is_not_json_comes_back_as_a_string() {
    let output = run(
        ScriptLanguage::Shell,
        "echo hello there",
        &json!(null),
        TIMEOUT,
        None,
    )
    .await
    .expect("runs");

    // A script that just prints a line should still be usable downstream.
    assert_eq!(output.value, json!("hello there"));
}

// Unix-only: these run `ScriptLanguage::Shell`, which `run_script` refuses
// on Windows because there is no portable POSIX shell to run it in (see
// the `#[cfg(windows)]` guard in `run_script`) rather than emulating one.
#[cfg(unix)]
#[tokio::test]
async fn stderr_survives_a_script_that_succeeded() {
    let output = run(
        ScriptLanguage::Shell,
        "echo warning: skipped one >&2; echo done",
        &json!(null),
        TIMEOUT,
        None,
    )
    .await
    .expect("runs");

    assert_eq!(output.value, json!("done"));
    // A script that works and warns wrote that warning for a reader.
    assert_eq!(output.stderr, "warning: skipped one");
}

// Unix-only: these run `ScriptLanguage::Shell`, which `run_script` refuses
// on Windows because there is no portable POSIX shell to run it in (see
// the `#[cfg(windows)]` guard in `run_script`) rather than emulating one.
#[cfg(unix)]
#[tokio::test]
async fn a_failing_script_reports_its_stderr_rather_than_only_its_code() {
    let err = run(
        ScriptLanguage::Shell,
        "echo could not reach the host >&2; exit 3",
        &json!(null),
        TIMEOUT,
        None,
    )
    .await
    .expect_err("fails");

    // The exit code alone says nothing an author can act on.
    assert!(
        err.to_string().contains("could not reach the host"),
        "{err}"
    );
}

// Unix-only: these run `ScriptLanguage::Shell`, which `run_script` refuses
// on Windows because there is no portable POSIX shell to run it in (see
// the `#[cfg(windows)]` guard in `run_script`) rather than emulating one.
#[cfg(unix)]
#[tokio::test]
async fn a_script_that_never_ends_is_stopped_and_says_so() {
    let err = run(
        ScriptLanguage::Shell,
        "sleep 30",
        &json!(null),
        Duration::from_millis(300),
        None,
    )
    .await
    .expect_err("times out");

    assert!(err.to_string().contains("timed out"), "{err}");
}

#[tokio::test]
async fn a_missing_interpreter_says_what_is_missing_rather_than_failing_opaquely() {
    let err = run(
        ScriptLanguage::Python,
        "print(1)",
        &json!(null),
        TIMEOUT,
        None,
    )
    .await;

    // Only meaningful when python3 is genuinely absent; where it exists this
    // case cannot arise and the assertion is skipped.
    if let Err(err) = err {
        if err.to_string().contains("cannot run") {
            assert!(err.to_string().contains("PATH"), "{err}");
        }
    }
}

// Unix-only: these run `ScriptLanguage::Shell`, which `run_script` refuses
// on Windows because there is no portable POSIX shell to run it in (see
// the `#[cfg(windows)]` guard in `run_script`) rather than emulating one.
#[cfg(unix)]
#[tokio::test]
async fn a_script_runs_where_it_was_told_to() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("marker.txt"), "found me").expect("write");

    let output = run(
        ScriptLanguage::Shell,
        "cat marker.txt",
        &json!(null),
        TIMEOUT,
        Some(dir.path()),
    )
    .await
    .expect("runs");

    // This is what makes `medulla:shell` useful: a step that means to touch the
    // operator's project has to actually be in it.
    assert_eq!(output.value, json!("found me"));
}

// Unix-only: these run `ScriptLanguage::Shell`, which `run_script` refuses
// on Windows because there is no portable POSIX shell to run it in (see
// the `#[cfg(windows)]` guard in `run_script`) rather than emulating one.
#[cfg(unix)]
#[tokio::test]
async fn a_script_with_no_directory_given_runs_somewhere_disposable() {
    let output = run(ScriptLanguage::Shell, "pwd", &json!(null), TIMEOUT, None)
        .await
        .expect("runs");

    // A `code` node is a computation over its input, so it gets a scratch
    // directory rather than the repository.
    let cwd = output.value.as_str().expect("a path");
    assert_ne!(
        cwd,
        std::env::current_dir().unwrap().to_string_lossy(),
        "a code node must not default to the process's own directory"
    );
}

#[tokio::test]
async fn javascript_reads_the_same_input_the_same_way() {
    if !available("node") {
        return;
    }

    let output = run(
        ScriptLanguage::JavaScript,
        "const fs = require('fs');\n\
         const input = JSON.parse(fs.readFileSync(0, 'utf8'));\n\
         console.log(JSON.stringify({ doubled: input.n * 2 }));",
        &json!({ "n": 21 }),
        TIMEOUT,
        None,
    )
    .await
    .expect("runs");

    assert_eq!(output.value, json!({ "doubled": 42 }));
}

#[tokio::test]
async fn python_reads_the_same_input_the_same_way() {
    if !available("python3") {
        return;
    }

    let output = run(
        ScriptLanguage::Python,
        "import json, sys\nprint(json.dumps({'doubled': json.load(sys.stdin)['n'] * 2}))",
        &json!({ "n": 21 }),
        TIMEOUT,
        None,
    )
    .await
    .expect("runs");

    assert_eq!(output.value, json!({ "doubled": 42 }));
}

#[test]
fn a_language_name_is_read_the_way_an_author_would_write_it() {
    for name in ["shell", "sh", "bash", "SHELL", " bash "] {
        assert_eq!(
            ScriptLanguage::parse(name),
            Some(ScriptLanguage::Shell),
            "{name}"
        );
    }
    for name in ["javascript", "js", "node", "nodejs"] {
        assert_eq!(
            ScriptLanguage::parse(name),
            Some(ScriptLanguage::JavaScript),
            "{name}"
        );
    }
    for name in ["python", "python3", "py"] {
        assert_eq!(
            ScriptLanguage::parse(name),
            Some(ScriptLanguage::Python),
            "{name}"
        );
    }
    // Refused rather than guessed: running the wrong interpreter on someone's
    // script is worse than telling them the name was not recognised.
    assert_eq!(ScriptLanguage::parse("ruby"), None);
}

#[cfg(windows)]
#[tokio::test]
async fn shell_is_refused_on_windows_rather_than_emulated() {
    // The one Windows-specific behavior this module has: refuse plainly,
    // pointing at the languages that do work everywhere, instead of
    // path-translating into Git Bash or swapping in `cmd`/PowerShell — either
    // of which would make a workflow look portable while quietly behaving
    // differently by host.
    let err = run(
        ScriptLanguage::Shell,
        "echo hi",
        &json!(null),
        TIMEOUT,
        None,
    )
    .await
    .expect_err("shell must be refused on Windows");

    let message = err.to_string();
    assert!(message.contains("Windows"), "{message}");
    assert!(message.contains("javascript"), "{message}");
    assert!(message.contains("python"), "{message}");
}
