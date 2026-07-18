//! Mock coding-agent CLI harness for daemon e2e tests.
//!
//! This is a richer successor to `fake_provider`'s ad-hoc shell snippets: a
//! single [`MockCli`] builder renders a `/bin/sh` script that emits the *exact*
//! streaming-JSONL shapes the daemon mappers ([`medulla::daemon::mappers`])
//! parse for each provider — claude `-p --output-format stream-json`, codex
//! `exec --json`, and opencode `run --format json`. The daemon's real spawn path
//! ([`medulla::daemon::providers::run_provider_task`]) runs them through the
//! `TINYPLACE_*_BIN` env overrides.
//!
//! A mock is a sequence of high-level [`Step`]s (thinking, agent messages, tool
//! call/result pairs, provider errors, garbage lines) plus a terminal behavior
//! (clean exit, non-zero exit with a stderr tail, or hang-until-killed for the
//! idle watchdog). Each step is lowered to the provider-specific record shape, so
//! the same scenario can be replayed against any of the three providers.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Which provider transcript shape a mock emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockProvider {
    Claude,
    Codex,
    Opencode,
}

impl MockProvider {
    fn bin_name(self) -> &'static str {
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
enum Terminal {
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
    provider: MockProvider,
    steps: Vec<Step>,
    terminal: Terminal,
    session_log: Option<SessionLogSpec>,
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

    /// The provider-appropriate reply line for `text` (result line for claude,
    /// an agent message for codex, a text part for opencode).
    fn reply_line(&self, text: &str) -> String {
        match self.provider {
            MockProvider::Claude => json!({ "type": "result", "result": text }).to_string(),
            MockProvider::Codex => codex_record(
                "event_msg",
                json!({ "type": "agent_message", "message": text }),
            ),
            MockProvider::Opencode => {
                opencode_record("text", json!({ "type": "text", "text": text }))
            }
        }
    }

    /// Render the executable `/bin/sh` script body.
    pub fn script(&self) -> String {
        if let Some(spec) = &self.session_log {
            return self.session_log_script(spec);
        }
        let mut out = String::from("#!/bin/sh\n");
        for step in &self.steps {
            for line in self.lower(step) {
                out.push_str(&emit_line(&line));
            }
        }
        match &self.terminal {
            Terminal::ClaudeResult(reply) => {
                let line = json!({ "type": "result", "result": reply }).to_string();
                out.push_str(&emit_line(&line));
            }
            Terminal::Exit => {}
            Terminal::Fail { code, stderr } => {
                out.push_str(&format!("printf '%s\\n' {} >&2\n", sh_quote(stderr)));
                out.push_str(&format!("exit {code}\n"));
            }
            Terminal::Hang => {
                // No sleep (PATH-independent): block reading stdin that never comes.
                out.push_str("while read _line; do :; done\n");
                out.push_str("cat >/dev/null\n");
            }
            Terminal::StdinEcho { provider_reply } => {
                out.push_str("read line\n");
                let line = match self.provider {
                    MockProvider::Claude => {
                        r#"printf '{"type":"result","result":"got: %s"}\n' "$line""#.to_string()
                    }
                    MockProvider::Codex if *provider_reply => {
                        r#"printf '{"type":"event_msg","payload":{"type":"agent_message","message":"got: %s"}}\n' "$line""#.to_string()
                    }
                    _ => {
                        r#"printf '{"type":"text","part":{"type":"text","text":"got: %s"}}\n' "$line""#.to_string()
                    }
                };
                out.push_str(&line);
                out.push('\n');
            }
            Terminal::FlakyLock(reply) => {
                out.push_str("MARKER=\"$0.lock\"\n");
                out.push_str(
                    "if [ ! -f \"$MARKER\" ]; then : > \"$MARKER\"; printf '%s\\n' 'Error: database is locked' >&2; exit 1; fi\n",
                );
                out.push_str(&emit_line(&self.reply_line(reply)));
            }
        }
        out
    }

    /// Render a `/bin/sh` script that writes its transcript to a session-log file
    /// (for the wrapper tailer) rather than streaming to stdout.
    fn session_log_script(&self, spec: &SessionLogSpec) -> String {
        let mut out = String::from("#!/bin/sh\n");
        out.push_str(&format!("LOG={}\n", sh_quote(&spec.path.to_string_lossy())));
        out.push_str("mkdir -p \"$(dirname \"$LOG\")\"\n");
        // Head record carrying the session id + cwd so the wrapper can anchor it.
        let head = match self.provider {
            MockProvider::Codex | MockProvider::Opencode => json!({
                "type": "session_meta",
                "timestamp": "2026-07-05T00:00:00.000Z",
                "payload": { "session_id": spec.session_id, "cwd": spec.cwd },
            })
            .to_string(),
            MockProvider::Claude => json!({
                "type": "summary",
                "timestamp": "2026-07-05T00:00:00.000Z",
                "sessionId": spec.session_id,
                "cwd": spec.cwd,
            })
            .to_string(),
        };
        out.push_str(&emit_log_line(&head));
        for step in &self.steps {
            for line in self.lower(step) {
                out.push_str(&emit_log_line(&line));
            }
        }
        if spec.echo_stdin {
            out.push_str("read line\n");
            let record = match self.provider {
                MockProvider::Claude => {
                    r#"printf '{"type":"assistant","timestamp":"2026-07-05T00:00:00.000Z","message":{"role":"assistant","content":[{"type":"text","text":"got: %s"}]}}\n' "$line" >> "$LOG""#
                }
                _ => {
                    r#"printf '{"type":"event_msg","timestamp":"2026-07-05T00:00:00.000Z","payload":{"type":"agent_message","message":"got: %s"}}\n' "$line" >> "$LOG""#
                }
            };
            out.push_str(record);
            out.push('\n');
        }
        if let Terminal::Fail { code, stderr } = &self.terminal {
            out.push_str(&format!("printf '%s\\n' {} >&2\n", sh_quote(stderr)));
            out.push_str(&format!("exit {code}\n"));
        }
        out
    }

    /// Lower one high-level step to zero or more provider-specific JSONL lines.
    fn lower(&self, step: &Step) -> Vec<String> {
        match self.provider {
            MockProvider::Claude => self.lower_claude(step),
            MockProvider::Codex => self.lower_codex(step),
            MockProvider::Opencode => self.lower_opencode(step),
        }
    }

    fn lower_claude(&self, step: &Step) -> Vec<String> {
        match step {
            Step::Prompt(text) => vec![claude_record(
                "user",
                json!({ "role": "user", "content": text }),
            )],
            Step::Thinking(text) => vec![claude_record(
                "assistant",
                json!({ "role": "assistant", "content": [{ "type": "thinking", "thinking": text }] }),
            )],
            Step::Message(text) => vec![claude_record(
                "assistant",
                json!({ "role": "assistant", "content": [{ "type": "text", "text": text }] }),
            )],
            Step::Tool {
                name,
                input,
                output,
                is_error,
            } => {
                let call_id = next_call_id();
                vec![
                    claude_record(
                        "assistant",
                        json!({ "role": "assistant", "content": [{
                            "type": "tool_use", "id": call_id, "name": name, "input": input,
                        }] }),
                    ),
                    claude_record(
                        "user",
                        json!({ "role": "user", "content": [{
                            "type": "tool_result", "tool_use_id": call_id,
                            "is_error": is_error, "content": output,
                        }] }),
                    ),
                ]
            }
            Step::Status { .. } => Vec::new(),
            Step::ProviderError(_) => Vec::new(),
            Step::Garbage(line) => vec![line.clone()],
            Step::Raw(line) => vec![line.clone()],
        }
    }

    fn lower_codex(&self, step: &Step) -> Vec<String> {
        match step {
            Step::Prompt(text) => vec![codex_record(
                "event_msg",
                json!({ "type": "user_message", "message": text }),
            )],
            Step::Thinking(text) => vec![codex_record(
                "response_item",
                json!({ "type": "reasoning", "summary": [{ "type": "summary_text", "text": text }] }),
            )],
            Step::Message(text) => vec![codex_record(
                "event_msg",
                json!({ "type": "agent_message", "message": text }),
            )],
            Step::Tool {
                name,
                input,
                output,
                is_error,
            } => {
                let call_id = next_call_id();
                // Codex serializes arguments as a JSON *string*.
                let arguments = serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string());
                vec![
                    codex_record(
                        "response_item",
                        json!({
                            "type": "function_call", "name": name,
                            "call_id": call_id, "arguments": arguments,
                        }),
                    ),
                    codex_record(
                        "response_item",
                        json!({
                            "type": "function_call_output", "call_id": call_id,
                            "output": output, "success": !is_error,
                        }),
                    ),
                ]
            }
            Step::Status { running } => vec![codex_record(
                "event_msg",
                json!({ "type": if *running { "task_started" } else { "task_complete" } }),
            )],
            Step::ProviderError(_) => Vec::new(),
            Step::Garbage(line) => vec![line.clone()],
            Step::Raw(line) => vec![line.clone()],
        }
    }

    fn lower_opencode(&self, step: &Step) -> Vec<String> {
        match step {
            // OpenCode's flat run format carries no user echo.
            Step::Prompt(_) => Vec::new(),
            Step::Thinking(text) => vec![opencode_record(
                "reasoning",
                json!({ "type": "reasoning", "text": text }),
            )],
            Step::Message(text) => vec![opencode_record(
                "text",
                json!({ "type": "text", "text": text }),
            )],
            Step::Tool {
                name,
                input,
                output,
                is_error,
            } => {
                let call_id = next_call_id();
                let status = if *is_error { "error" } else { "completed" };
                vec![
                    opencode_record(
                        "tool",
                        json!({
                            "type": "tool", "tool": name, "callID": call_id,
                            "state": { "status": "running", "input": input },
                        }),
                    ),
                    opencode_record(
                        "tool",
                        json!({
                            "type": "tool", "tool": name, "callID": call_id,
                            "state": { "status": status, "output": output },
                        }),
                    ),
                ]
            }
            Step::Status { .. } => Vec::new(),
            Step::ProviderError(message) => vec![json!({
                "type": "error",
                "error": { "name": "ProviderError", "data": { "message": message } },
            })
            .to_string()],
            Step::Garbage(line) => vec![line.clone()],
            Step::Raw(line) => vec![line.clone()],
        }
    }
}

// ── canned scenarios ─────────────────────────────────────────────────────────

/// A clean success: one message, terminated appropriately per provider.
pub fn success(provider: MockProvider, reply: &str) -> MockCli {
    let cli = MockCli::new(provider).message(reply);
    match provider {
        MockProvider::Claude => cli.claude_result(reply),
        _ => cli,
    }
}

/// Thinking + a tool call/result + a final message (rich status stream).
pub fn tool_workflow(provider: MockProvider, reply: &str) -> MockCli {
    let cli = MockCli::new(provider)
        .thinking("planning the work")
        .tool(
            "read",
            json!({ "file_path": "/a/b.rs" }),
            "file contents",
            false,
        )
        .message(reply);
    match provider {
        MockProvider::Claude => cli.claude_result(reply),
        _ => cli,
    }
}

/// Garbage + oversized lines the mapper drops, then a valid reply.
pub fn garbage_then_reply(provider: MockProvider, reply: &str) -> MockCli {
    let cli = MockCli::new(provider)
        .garbage("this is not json at all")
        .garbage("{ broken json ")
        .message(reply);
    match provider {
        MockProvider::Claude => cli.claude_result(reply),
        _ => cli,
    }
}

/// A hang-until-killed mock (idle watchdog exercise); emits nothing first.
pub fn hang(provider: MockProvider) -> MockCli {
    MockCli::new(provider).hang()
}

/// Emits one event, then hangs — exercises the deadline-reset then kill path.
pub fn event_then_hang(provider: MockProvider) -> MockCli {
    MockCli::new(provider).message("starting").hang()
}

/// A non-zero exit carrying an auth-shaped stderr tail.
pub fn auth_failure(provider: MockProvider) -> MockCli {
    MockCli::new(provider).fail(1, "unexpected server error: unauthorized (401)")
}

/// A capability self-report echoed through the provider's reply channel.
pub fn capabilities(provider: MockProvider, report_json: &str) -> MockCli {
    match provider {
        MockProvider::Claude => MockCli::new(provider).claude_result(report_json),
        _ => MockCli::new(provider).message(report_json),
    }
}

// ── record builders ──────────────────────────────────────────────────────────

fn claude_record(record_type: &str, message: Value) -> String {
    json!({
        "type": record_type,
        "timestamp": "2026-07-05T00:00:00Z",
        "message": message,
    })
    .to_string()
}

fn codex_record(record_type: &str, payload: Value) -> String {
    json!({
        "type": record_type,
        "timestamp": "2026-07-05T00:00:00Z",
        "payload": payload,
    })
    .to_string()
}

fn opencode_record(record_type: &str, part: Value) -> String {
    json!({
        "type": record_type,
        "timestamp": "2026-07-05T00:00:00Z",
        "part": part,
    })
    .to_string()
}

fn next_call_id() -> String {
    format!("call_{}", COUNTER.fetch_add(1, Ordering::SeqCst))
}

/// Emit a `printf` that writes `line` verbatim followed by a newline. The line
/// (JSON, so `%`-free of format directives) is single-quote shell-escaped.
fn emit_line(line: &str) -> String {
    format!("printf '%s\\n' {}\n", sh_quote(line))
}

/// Emit a `printf` appending `line` (plus newline) to the `$LOG` session file.
fn emit_log_line(line: &str) -> String {
    format!("printf '%s\\n' {} >> \"$LOG\"\n", sh_quote(line))
}

/// Single-quote a string for POSIX sh, escaping embedded single quotes.
fn sh_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

// ── temp-dir + install helpers ───────────────────────────────────────────────

/// A temp directory removed on drop; hosts the generated mock binaries.
pub struct MockDir {
    path: PathBuf,
}

impl MockDir {
    pub fn new() -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "medulla-mock-{}-{}-{id}",
            std::process::id(),
            now_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        MockDir { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn path_str(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    /// Write a mock CLI as an executable and return its absolute path.
    pub fn install(&self, mock: &MockCli) -> String {
        let name = format!("{}-{}", mock.provider.bin_name(), next_call_id());
        let path = self.path.join(&name);
        std::fs::write(&path, mock.script()).unwrap();
        set_executable(&path);
        path.to_string_lossy().into_owned()
    }

    /// Install `mock` and return the `TINYPLACE_*_BIN` env override for it.
    pub fn env_for(&self, mock: &MockCli) -> HashMap<String, String> {
        let bin = self.install(mock);
        provider_env(&[(mock.provider.bin_env(), &bin)])
    }
}

impl Default for MockDir {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for MockDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A provider env map carrying host `PATH` plus each `TINYPLACE_*_BIN` override.
pub fn provider_env(overrides: &[(&str, &str)]) -> HashMap<String, String> {
    let mut env = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    for (key, value) in overrides {
        env.insert((*key).to_string(), (*value).to_string());
    }
    env
}

#[cfg(unix)]
fn set_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) {}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}
