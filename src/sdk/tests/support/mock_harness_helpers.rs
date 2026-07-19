//! Canned scenarios, provider record builders, and temp-dir install helpers for
//! the mock harness.
//!
//! Groups three cohesive concerns the tests and the `script` module lean on:
//! ready-made [`MockCli`] scenarios (`success`, `tool_workflow`, …); the
//! provider-shaped JSONL record builders + shell-emit helpers used while
//! lowering steps; and [`MockDir`], a temp directory that installs a rendered
//! mock as an executable and hands back the `TINYPLACE_*_BIN` env override.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

use super::types::*;

static COUNTER: AtomicU64 = AtomicU64::new(0);

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

pub fn claude_record(record_type: &str, message: Value) -> String {
    json!({
        "type": record_type,
        "timestamp": "2026-07-05T00:00:00Z",
        "message": message,
    })
    .to_string()
}

pub fn codex_record(record_type: &str, payload: Value) -> String {
    json!({
        "type": record_type,
        "timestamp": "2026-07-05T00:00:00Z",
        "payload": payload,
    })
    .to_string()
}

pub fn opencode_record(record_type: &str, part: Value) -> String {
    json!({
        "type": record_type,
        "timestamp": "2026-07-05T00:00:00Z",
        "part": part,
    })
    .to_string()
}

pub fn next_call_id() -> String {
    format!("call_{}", COUNTER.fetch_add(1, Ordering::SeqCst))
}

/// Emit a `printf` that writes `line` verbatim followed by a newline. The line
/// (JSON, so `%`-free of format directives) is single-quote shell-escaped.
pub fn emit_line(line: &str) -> String {
    format!("printf '%s\\n' {}\n", sh_quote(line))
}

/// Emit a `printf` appending `line` (plus newline) to the `$LOG` session file.
pub fn emit_log_line(line: &str) -> String {
    format!("printf '%s\\n' {} >> \"$LOG\"\n", sh_quote(line))
}

/// Single-quote a string for POSIX sh, escaping embedded single quotes.
pub fn sh_quote(value: &str) -> String {
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
