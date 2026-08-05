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
//! What *is* checked, at the boundary above this one, is where a script may come
//! from and where it may run: see [`super::script_policy`].
//! `workflows.allowCode` is on by default for locally authored workflows, but
//! can be explicitly disabled when definitions come from an untrusted source.
//! Everything here is about making a trusted script *work correctly*, not about
//! containing an untrusted one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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
    /// POSIX shell, run with `bash` unless an [`Interpreter`] says otherwise.
    Shell,
}

impl ScriptLanguage {
    /// The interpreter and the extension its file wants.
    fn program(self) -> (&'static str, &'static str) {
        match self {
            Self::JavaScript => ("node", "js"),
            Self::Python => ("python3", "py"),
            Self::Shell => (DEFAULT_SHELL, "sh"),
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

/// The shell a `shell` script runs under when nothing chooses another.
///
/// Not the operator's login shell: an existing workflow's script was written
/// against *this*, and quietly re-running it under `fish` or `dash` because that
/// is what `$SHELL` happens to say would break it in ways that look like the
/// script's fault. Tracking the login shell is available, but it is opted into —
/// see [`Interpreter::resolve`].
pub const DEFAULT_SHELL: &str = "bash";

/// The configured value that means "whatever the operator's login shell is".
pub const USER_SHELL: &str = "user";

/// The program a script runs under, and the arguments that precede its path.
///
/// Exists so the shell is a decision rather than a constant. The reason an
/// operator reaches for it is almost always the same one: their own functions,
/// aliases, and `PATH` live in `~/.zshrc`, and a script run as
/// `bash <path>` — non-login, non-interactive — sees none of it. Naming `zsh`
/// with `args: ["-l"]` is what puts those back in scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interpreter {
    /// The program to spawn: a bare name resolved on `PATH`, or an absolute
    /// path.
    pub program: String,
    /// Arguments passed before the script path — `["-l"]` for a login shell,
    /// `["-l", "-i"]` to also get aliases, which are not exported.
    pub args: Vec<String>,
}

impl Interpreter {
    /// The default shell, with no leading arguments.
    #[must_use]
    pub fn default_shell() -> Self {
        Self {
            program: DEFAULT_SHELL.to_string(),
            args: Vec::new(),
        }
    }

    /// Choose the interpreter an operator's configuration asks for.
    ///
    /// `configured` is `workflows.shell` (or a node's own `args.shell`):
    ///
    /// - empty — [`DEFAULT_SHELL`], so an unconfigured host is unchanged.
    /// - [`USER_SHELL`] — the login shell `login_shell` reports, falling back to
    ///   [`DEFAULT_SHELL`] when the environment names none. This is the opt-in
    ///   that makes scripts run under whatever the operator actually uses.
    /// - anything else — that program.
    ///
    /// `login_shell` is passed in rather than read from the environment here so
    /// the choice is a pure function of its inputs, testable without mutating
    /// process-global state.
    ///
    /// # Errors
    ///
    /// Refuses a program that is not a bare name or an absolute path, and any
    /// program or argument carrying an interior NUL. A relative path is refused
    /// rather than resolved because what it would resolve *against* is the
    /// script's working directory, which the workflow author chose — so
    /// `workflows.shell = "./sh"` would let a graph decide which binary the
    /// operator's own configuration named.
    pub fn resolve(configured: &str, args: &[String], login_shell: Option<&str>) -> Result<Self> {
        let configured = configured.trim();
        let program = match configured {
            "" => DEFAULT_SHELL,
            USER_SHELL => login_shell
                .map(str::trim)
                .filter(|shell| !shell.is_empty())
                .unwrap_or(DEFAULT_SHELL),
            other => other,
        };
        Self::validated(program, args)
    }

    /// An interpreter from an already-chosen program name, checked.
    ///
    /// # Errors
    ///
    /// As [`resolve`](Self::resolve).
    pub fn validated(program: &str, args: &[String]) -> Result<Self> {
        let program = program.trim();
        if program.is_empty() {
            return Err(refused("the interpreter must not be empty"));
        }
        if program.contains('\0') {
            return Err(refused(format!(
                "the interpreter {program:?} contains a NUL byte, which cannot be passed to a \
                 process"
            )));
        }
        // `is_separator` rather than `MAIN_SEPARATOR`: Windows accepts `/` as
        // well as `\`, so matching only the platform's *preferred* separator
        // would wave `./sh` straight through on the one platform where two
        // spellings exist. It stays exact on unix, where `\` is an ordinary
        // filename character.
        if program.chars().any(std::path::is_separator) && !Path::new(program).is_absolute() {
            return Err(refused(format!(
                "the interpreter {program:?} is a relative path; name a program on PATH (\"zsh\") \
                 or give an absolute path (\"/bin/zsh\")"
            )));
        }
        for arg in args {
            if arg.contains('\0') {
                return Err(refused(format!(
                    "the interpreter argument {arg:?} contains a NUL byte, which cannot be passed \
                     to a process"
                )));
            }
        }
        Ok(Self {
            program: program.to_string(),
            args: args.to_vec(),
        })
    }

    /// The operator's login shell, as the environment reports it.
    ///
    /// `$SHELL` only — no `/etc/passwd` lookup, because the environment is what
    /// a daemon started from a login session actually carries, and a passwd
    /// entry would disagree with it exactly when a user has changed shells
    /// without re-logging in.
    #[must_use]
    pub fn login_shell() -> Option<String> {
        std::env::var("SHELL").ok()
    }
}

/// A refusal about the interpreter, prefixed so a run record says what refused.
fn refused(message: impl AsRef<str>) -> EngineError {
    EngineError::Capability(format!("script: {}", message.as_ref()))
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

/// What to run: source this host stages, or a file that already exists.
#[derive(Debug, Clone, Copy)]
pub enum ScriptSource<'a> {
    /// Source text, written to a temporary file before it is run.
    Inline(&'a str),
    /// An existing script file.
    ///
    /// Whether a workflow may reach this path is decided *before* it gets here,
    /// by [`super::script_policy`]; nothing below re-checks it.
    File(&'a Path),
}

/// Everything one script run needs.
///
/// A struct rather than six positional arguments, two of which are paths and
/// three of which are optional — an order a caller would eventually get wrong
/// without the compiler noticing.
#[derive(Debug, Clone, Copy)]
pub struct ScriptRequest<'a> {
    /// The language the script is written in. Decides the interpreter, unless
    /// `interpreter` names one, and always decides the staged file's extension.
    pub language: ScriptLanguage,
    /// The interpreter to run under, overriding the one `language` implies.
    ///
    /// `None` keeps the language's own program, which is what a `code` node
    /// wants: its `javascript` and `python` are the contract, not a preference.
    pub interpreter: Option<&'a Interpreter>,
    /// The script itself.
    pub source: ScriptSource<'a>,
    /// The JSON handed to the script on stdin, and written to `argv[1]`.
    pub input: &'a Value,
    /// How long the script may run before it is abandoned.
    pub timeout: Duration,
    /// The directory to run in. `None` uses the temporary directory holding the
    /// staged script — right for a pure computation, wrong for anything that
    /// means to touch the operator's project.
    pub cwd: Option<&'a Path>,
    /// Environment variables layered over the inherited environment.
    pub env: &'a BTreeMap<String, String>,
}

impl<'a> ScriptRequest<'a> {
    /// A request with no working directory and no extra environment — the shape
    /// a pure computation over `input` wants.
    pub fn plain(
        language: ScriptLanguage,
        source: &'a str,
        input: &'a Value,
        timeout: Duration,
        env: &'a BTreeMap<String, String>,
    ) -> Self {
        Self {
            language,
            interpreter: None,
            source: ScriptSource::Inline(source),
            input,
            timeout,
            cwd: None,
            env,
        }
    }
}

/// Run the script `request` describes.
///
/// # Errors
///
/// Fails when the interpreter is missing, the script exits non-zero, or it
/// outlives `request.timeout`. The message carries the interpreter's own stderr,
/// which is the only thing that says what actually went wrong.
pub async fn run_script(request: ScriptRequest<'_>) -> Result<ScriptOutput> {
    use tokio::io::AsyncWriteExt;

    let ScriptRequest {
        language,
        interpreter,
        source,
        input,
        timeout,
        cwd,
        env,
    } = request;

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

    // The extension always comes from the language; only the program is
    // negotiable. A `.sh` staged for `zsh` is still a shell script.
    let (default_program, extension) = language.program();
    let (program, leading_args) = match interpreter {
        Some(chosen) => (chosen.program.as_str(), chosen.args.as_slice()),
        None => (default_program, &[][..]),
    };
    let dir =
        tempfile::tempdir().map_err(|err| EngineError::Capability(format!("script: {err}")))?;

    let script: PathBuf = match source {
        ScriptSource::Inline(source) => {
            let staged = dir.path().join(format!("script.{extension}"));
            std::fs::write(&staged, source)
                .map_err(|err| EngineError::Capability(format!("script: {err}")))?;
            staged
        }
        ScriptSource::File(path) => path.to_path_buf(),
    };

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
        .args(leading_args)
        .arg(&script)
        .arg(&input_path)
        .env("MEDULLA_INPUT", &input_path)
        // Layered after `MEDULLA_INPUT` so a workflow's own declaration wins
        // over the inherited value of the same name — that is what declaring
        // one is for.
        .envs(env)
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
