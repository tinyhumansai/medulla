//! Running a script out-of-process, for `code` nodes and the `medulla:shell`
//! tool.
//!
//! One executor behind both, so a workflow author learns one calling convention
//! rather than two. What it does is deliberately small: write the source to a
//! temporary file, run it, read stdout.
//!
//! # The calling convention
//!
//! A script gets its input **on stdin, as JSON**, and returns its result **on
//! stdout**. Stdout that parses as JSON becomes structured output; anything else
//! becomes a string, so a script that prints one line is still usable.
//!
//! Stdin rather than an argument because it is the one channel every language
//! reads the same way — `JSON.parse(require('fs').readFileSync(0,'utf8'))`,
//! `json.load(sys.stdin)`, `cat`. The input is *also* written to a file whose
//! path is `argv[1]` and `$MEDULLA_INPUT`, because a large payload through a
//! pipe is awkward in shell and a path is not.
//!
//! # What this is not
//!
//! Not a sandbox. The child inherits this process's environment and privileges,
//! and the only boundary is a temporary working directory, which is not one.
//! That is why `workflows.allowCode` is off by default: enabling it is an
//! operator's explicit decision to trust whoever wrote the workflow. Everything
//! here is about making a trusted script *work correctly*, not about containing
//! an untrusted one.

use std::time::Duration;

use serde_json::Value;
use tinyflows::error::{EngineError, Result};

/// A language this host can execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptLanguage {
    /// Node.js.
    JavaScript,
    /// CPython 3.
    Python,
    /// POSIX shell, run with `bash`.
    Shell,
}

impl ScriptLanguage {
    /// The interpreter and the extension its file wants.
    fn program(self) -> (&'static str, &'static str) {
        match self {
            Self::JavaScript => ("node", "js"),
            Self::Python => ("python3", "py"),
            Self::Shell => ("bash", "sh"),
        }
    }

    /// The name an author writes in a node's config.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::JavaScript => "javascript",
            Self::Python => "python",
            Self::Shell => "shell",
        }
    }

    /// Parse the name an author wrote, accepting the obvious spellings.
    ///
    /// Forgiving on purpose: an author who writes `bash`, `sh`, `js`, or `py`
    /// meant something unambiguous, and refusing it teaches nothing.
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "javascript" | "js" | "node" | "nodejs" => Some(Self::JavaScript),
            "python" | "python3" | "py" => Some(Self::Python),
            "shell" | "sh" | "bash" => Some(Self::Shell),
            _ => None,
        }
    }

    /// Every spelling an author may write, for an error that teaches.
    pub const NAMES: [&'static str; 3] = ["javascript", "python", "shell"];
}

impl From<tinyflows::caps::CodeLanguage> for ScriptLanguage {
    fn from(language: tinyflows::caps::CodeLanguage) -> Self {
        match language {
            tinyflows::caps::CodeLanguage::JavaScript => Self::JavaScript,
            tinyflows::caps::CodeLanguage::Python => Self::Python,
        }
    }
}

/// What a script run produced.
#[derive(Debug)]
pub struct ScriptOutput {
    /// Stdout, parsed as JSON when it is JSON.
    pub value: Value,
    /// Stderr, kept whether or not the script succeeded.
    ///
    /// A script that works and warns is the normal case, and discarding what it
    /// said would hide the one thing its author wrote for a reader.
    pub stderr: String,
}

/// Run `source` in `language` with `input`, bounded by `timeout`.
///
/// `cwd`, when given, is the directory the script runs in. `None` uses the
/// temporary directory holding the script — right for a pure computation, wrong
/// for anything that means to touch the operator's project.
///
/// # Errors
///
/// Fails when the interpreter is missing, the script exits non-zero, or it
/// outlives `timeout`. The message carries the interpreter's own stderr, which
/// is the only thing that says what actually went wrong.
pub async fn run_script(
    language: ScriptLanguage,
    source: &str,
    input: &Value,
    timeout: Duration,
    cwd: Option<&std::path::Path>,
) -> Result<ScriptOutput> {
    use tokio::io::AsyncWriteExt;

    // Refused rather than emulated. `argv[1]` and `MEDULLA_INPUT` are real
    // filesystem paths this host wrote (`C:\...` on Windows), and Git Bash — the
    // only `bash` a Windows host is likely to have — cannot open a Windows path
    // without translating it, which is exactly the kind of per-platform
    // reinterpretation that would make a workflow look portable while quietly
    // behaving differently by host. `javascript` and `python` need no such
    // translation and stay available everywhere their interpreter is.
    #[cfg(windows)]
    if language == ScriptLanguage::Shell {
        return Err(EngineError::Capability(
            "script: shell scripts are not supported on Windows (no portable POSIX shell to \
             run them in); use language: \"javascript\" or \"python\" instead"
                .to_string(),
        ));
    }

    let (program, extension) = language.program();
    let dir =
        tempfile::tempdir().map_err(|err| EngineError::Capability(format!("script: {err}")))?;

    let script = dir.path().join(format!("script.{extension}"));
    std::fs::write(&script, source)
        .map_err(|err| EngineError::Capability(format!("script: {err}")))?;

    // The input reaches the script two ways because the languages want
    // different ones: a pipe reads naturally in node and python, a path reads
    // naturally in shell. Writing both costs one small file.
    let input_path = dir.path().join("input.json");
    let body = serde_json::to_vec(input)
        .map_err(|err| EngineError::Capability(format!("script: {err}")))?;
    std::fs::write(&input_path, &body)
        .map_err(|err| EngineError::Capability(format!("script: {err}")))?;

    let mut command = tokio::process::Command::new(program);
    command
        .arg(&script)
        .arg(&input_path)
        .env("MEDULLA_INPUT", &input_path)
        .current_dir(cwd.unwrap_or_else(|| dir.path()))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // Tokio does not kill a child when its future is dropped, so a timeout
        // would otherwise leave an infinite script running forever with nothing
        // holding a handle to it.
        .kill_on_drop(true);

    let mut child = command.spawn().map_err(|err| {
        EngineError::Capability(format!(
            "script: cannot run `{program}` ({err}). Is it installed and on PATH?"
        ))
    })?;

    // Writing stdin and draining stdout/stderr happen concurrently, not one
    // after the other: a script that prints before it finishes reading stdin
    // fills its stdout pipe while this side is still blocked in `write_all` on
    // stdin, and neither side would ever unblock the other — a real deadlock,
    // not just a slow path, for any input near the OS pipe buffer size. The
    // writer runs on its own task so `wait_with_output` starts reading
    // immediately; if the child exits without reading all of stdin, the pipe
    // simply closes underneath the writer, which surfaces as a write error we
    // ignore (the exit status and stderr are the story in that case, not this).
    let mut stdin = child.stdin.take();
    let writer = tokio::spawn(async move {
        if let Some(mut stdin) = stdin.take() {
            let _ = stdin.write_all(&body).await;
            let _ = stdin.shutdown().await;
        }
    });
    let output = tokio::time::timeout(timeout, async {
        let output = child.wait_with_output().await;
        // Joined so a slow writer is still bounded by `timeout` above, not left
        // running past the point this function returns.
        let _ = writer.await;
        output
    })
    .await
    .map_err(|_| {
        EngineError::Capability(format!("script: timed out after {}s", timeout.as_secs()))
    })?
    .map_err(|err| EngineError::Capability(format!("script: {program}: {err}")))?;

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !output.status.success() {
        return Err(EngineError::Capability(format!(
            "script: {program} exited with {}{}",
            output.status,
            if stderr.is_empty() {
                String::new()
            } else {
                format!(": {stderr}")
            }
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(ScriptOutput {
        // Structured when the script printed JSON, the raw text otherwise — a
        // script that just prints a line should still be usable downstream.
        value: serde_json::from_str(&stdout).unwrap_or(Value::String(stdout)),
        stderr,
    })
}

#[cfg(test)]
#[path = "script_tests.rs"]
mod tests;
