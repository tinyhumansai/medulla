//! Core data types and the [`MockCli`] builder surface for the mock harness.
//!
//! Defines the provider selector ([`MockProvider`]), the high-level transcript
//! [`Step`]s, the process [`Terminal`] behavior, the session-log spec
//! ([`SessionLogSpec`]), and the composable [`MockCli`] builder. Script
//! rendering lives in the sibling `script` module; canned scenarios, record
//! builders, and temp-dir install helpers live in `helpers`.

#![allow(dead_code)]

use std::path::PathBuf;

use serde_json::Value;

/// Which provider transcript shape a mock emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockProvider {
    Claude,
    Codex,
    Opencode,
}

impl MockProvider {
    pub fn bin_name(self) -> &'static str {
        match self {
            MockProvider::Claude => "claude",
            MockProvider::Codex => "codex",
            MockProvider::Opencode => "opencode",
        }
    }

    /// The `TINYPLACE_*_BIN` override key the daemon honors for this provider.
    pub fn bin_env(self) -> &'static str {
        match self {
            MockProvider::Claude => "TINYPLACE_CLAUDE_BIN",
            MockProvider::Codex => "TINYPLACE_CODEX_BIN",
            MockProvider::Opencode => "TINYPLACE_OPENCODE_BIN",
        }
    }
}

/// One high-level transcript step, lowered per-provider into concrete records.
#[derive(Debug, Clone)]
pub enum Step {
    /// The (echoed) inbound user prompt.
    Prompt(String),
    /// Agent chain-of-thought / reasoning.
    Thinking(String),
    /// An assistant message chunk (accumulates into the reply fallback).
    Message(String),
    /// A provider-shaped token-usage record (claude result-style usage object,
    /// codex token_count event, opencode usage record).
    Usage { input: i64, output: i64 },
    /// A tool invocation followed by its result, as one call/result pair.
    Tool {
        name: String,
        /// A value carried as the tool input (drives the status `display`).
        input: Value,
        output: String,
        is_error: bool,
    },
    /// A codex task lifecycle marker (ignored by the other providers).
    Status { running: bool },
    /// A provider-level error record (opencode `error`; a no-op for the others).
    ProviderError(String),
    /// A non-JSON line the mapper must silently drop.
    Garbage(String),
    /// A verbatim line emitted as-is (already valid JSON for the provider).
    Raw(String),
}

/// How the mock process terminates after emitting its steps.
#[derive(Debug, Clone)]
pub enum Terminal {
    /// Print the claude `result` line (reply precedence), then exit 0.
    ClaudeResult(String),
    /// Exit cleanly.
    Exit,
    /// Print `stderr` to fd 2 and exit with `code`.
    Fail { code: i32, stderr: String },
    /// Block on stdin forever so the idle watchdog must SIGKILL it.
    Hang,
    /// Read one stdin line and echo it back as the final reply, then exit.
    StdinEcho { provider_reply: bool },
    /// Exit with a transient SQLite-lock error on the first invocation (using a
    /// self-adjacent marker file), then emit `reply` on the retry. Drives the
    /// opencode lock-retry backoff loop in `run_provider_task`.
    FlakyLock(String),
}

/// How a mock writes a session-log transcript (for the wrapper tailer), instead
/// of streaming records to stdout. The wrapper discovers this file, tails it, and
/// forwards each record as a v2 envelope.
#[derive(Debug, Clone)]
pub struct SessionLogSpec {
    /// Absolute path of the JSONL transcript to write.
    pub path: PathBuf,
    /// The harness session id recorded in the transcript head.
    pub session_id: String,
    /// The cwd recorded in the head (must match the wrapper's cwd for discovery).
    pub cwd: String,
    /// After emitting steps, read one stdin line and append a `got: <line>`
    /// record to the transcript — proving injected input reached the child.
    pub echo_stdin: bool,
}

/// A composable mock coding-agent CLI.
#[derive(Debug, Clone)]
pub struct MockCli {
    pub provider: MockProvider,
    pub steps: Vec<Step>,
    pub terminal: Terminal,
    pub session_log: Option<SessionLogSpec>,
}

impl MockCli {
    pub fn new(provider: MockProvider) -> Self {
        MockCli {
            provider,
            steps: Vec::new(),
            terminal: Terminal::Exit,
            session_log: None,
        }
    }

    /// Write the transcript to a session-log file (for the wrapper tailer) rather
    /// than streaming to stdout. A `session_meta`/head record carrying `id`+`cwd`
    /// is written first so the wrapper's discovery can anchor it.
    pub fn write_session_log(mut self, path: impl Into<PathBuf>, id: &str, cwd: &str) -> Self {
        self.session_log = Some(SessionLogSpec {
            path: path.into(),
            session_id: id.to_string(),
            cwd: cwd.to_string(),
            echo_stdin: false,
        });
        self
    }

    /// Read one stdin line after the steps and append a `got: <line>` record to
    /// the session log (requires [`Self::write_session_log`]).
    pub fn echo_stdin_to_log(mut self) -> Self {
        if let Some(spec) = self.session_log.as_mut() {
            spec.echo_stdin = true;
        }
        self
    }

    pub fn step(mut self, step: Step) -> Self {
        self.steps.push(step);
        self
    }

    pub fn thinking(self, text: &str) -> Self {
        self.step(Step::Thinking(text.to_string()))
    }

    pub fn message(self, text: &str) -> Self {
        self.step(Step::Message(text.to_string()))
    }

    /// Emit a provider-appropriate token-usage record.
    pub fn usage(self, input: i64, output: i64) -> Self {
        self.step(Step::Usage { input, output })
    }

    pub fn tool(self, name: &str, input: Value, output: &str, is_error: bool) -> Self {
        self.step(Step::Tool {
            name: name.to_string(),
            input,
            output: output.to_string(),
            is_error,
        })
    }

    pub fn garbage(self, line: &str) -> Self {
        self.step(Step::Garbage(line.to_string()))
    }

    /// Terminate with a claude `result` line whose text becomes the reply.
    pub fn claude_result(mut self, reply: &str) -> Self {
        self.terminal = Terminal::ClaudeResult(reply.to_string());
        self
    }

    /// Terminate with a non-zero exit and a stderr tail (drives the exit-code
    /// error branch + auth-hint annotation).
    pub fn fail(mut self, code: i32, stderr: &str) -> Self {
        self.terminal = Terminal::Fail {
            code,
            stderr: stderr.to_string(),
        };
        self
    }

    /// Block forever after emitting steps — the idle watchdog must kill it.
    pub fn hang(mut self) -> Self {
        self.terminal = Terminal::Hang;
        self
    }

    /// Read one stdin line and echo it into the final reply (proves `input`
    /// forwarding reached the child process).
    pub fn stdin_echo(mut self) -> Self {
        self.terminal = Terminal::StdinEcho {
            provider_reply: self.provider != MockProvider::Claude,
        };
        self
    }

    /// Fail once with a transient lock error, then succeed on retry.
    pub fn flaky_lock(mut self, reply: &str) -> Self {
        self.terminal = Terminal::FlakyLock(reply.to_string());
        self
    }
}
