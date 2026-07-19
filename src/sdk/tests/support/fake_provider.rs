//! Fake provider-CLI scaffolding for daemon e2e tests.
//!
//! Writes small executable shell scripts into a self-cleaning temp dir that emit
//! realistic provider JSONL on stdout (claude `stream-json`, codex `exec --json`,
//! opencode `run --format json`). The daemon's real spawn path
//! ([`medulla::daemon::providers::run_provider_task`]) runs them via the
//! `TINYPLACE_*_BIN` env overrides, so tests exercise process spawning, JSONL
//! mapping, and reply extraction without any real coding-agent CLI.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A temp directory removed on drop.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new() -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "medulla-e2e-{}-{}-{id}",
            std::process::id(),
            now_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        TempDir { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn path_str(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    /// Write `body` as an executable script and return its absolute path.
    pub fn write_script(&self, name: &str, body: &str) -> String {
        let path = self.path.join(name);
        std::fs::write(&path, body).unwrap();
        set_executable(&path);
        path.to_string_lossy().into_owned()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
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

/// Build a provider env map carrying the host `PATH` (so `/bin/sh` builtins and
/// any spawned tools resolve) plus a `TINYPLACE_*_BIN` override per entry.
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

/// A claude `stream-json` script: an assistant text line, then a final
/// `{"type":"result"}` line whose text becomes the reply (result precedence).
pub fn claude_script(agent_text: &str, result_text: &str) -> String {
    format!(
        "#!/bin/sh\n\
         printf '%s\\n' '{{\"type\":\"assistant\",\"timestamp\":\"2026-07-05T00:00:00Z\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"{agent}\"}}]}}}}'\n\
         printf '%s\\n' '{{\"type\":\"result\",\"result\":\"{result}\",\"usage\":{{\"input_tokens\":1234,\"output_tokens\":56}}}}'\n",
        agent = agent_text,
        result = result_text,
    )
}

/// An opencode `run --format json` script emitting a tool_call, tool_result, and
/// a final text part (the reply, since opencode has no result line).
pub fn opencode_script(reply_text: &str) -> String {
    format!(
        "#!/bin/sh\n\
         printf '%s\\n' '{{\"type\":\"tool\",\"part\":{{\"type\":\"tool\",\"tool\":\"read\",\"callID\":\"r1\",\"state\":{{\"status\":\"running\",\"input\":{{\"file_path\":\"/a/b.rs\"}}}}}}}}'\n\
         printf '%s\\n' '{{\"type\":\"tool\",\"part\":{{\"type\":\"tool\",\"tool\":\"read\",\"callID\":\"r1\",\"state\":{{\"status\":\"completed\",\"output\":\"file contents\"}}}}}}'\n\
         printf '%s\\n' '{{\"type\":\"text\",\"part\":{{\"type\":\"text\",\"text\":\"{reply}\"}}}}'\n",
        reply = reply_text,
    )
}

/// A claude script that echoes a strict-JSON capability self-report as its
/// result (used by the capabilities probe).
pub fn claude_capabilities_script() -> String {
    // The inner JSON quotes are escaped for the result string field.
    let report = r#"{\"tools\":[\"Bash\",\"Read\"],\"mcpServers\":[\"langfuse\"],\"accessibleDirs\":[\"/opt/extra\"],\"summary\":\"fake claude\"}"#;
    format!("#!/bin/sh\nprintf '%s\\n' '{{\"type\":\"result\",\"result\":\"{report}\"}}'\n")
}

/// A claude script that waits for one stdin line, then emits it back in the
/// result (used to prove `input` forwarding reached the child).
pub fn claude_stdin_echo_script() -> String {
    "#!/bin/sh\n\
     printf '%s\\n' '{\"type\":\"assistant\",\"timestamp\":\"2026-07-05T00:00:00Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"waiting\"}]}}'\n\
     read line\n\
     printf '{\"type\":\"result\",\"result\":\"got: %s\"}\\n' \"$line\"\n"
        .to_string()
}

/// A script that blocks on stdin forever, emitting nothing, so the idle watchdog
/// fires. No `sleep` (PATH-independent); the daemon SIGKILLs the child.
pub fn hanging_script() -> String {
    "#!/bin/sh\nread line\n".to_string()
}
