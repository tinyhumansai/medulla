//! The shell tool: run a command inside the turn's checkout.
//!
//! # The environment is explicit, never inherited
//!
//! The command runs under `env_clear()` plus a caller-supplied map — the same
//! discipline `daemon::providers::execute` applies to every spawned CLI
//! harness. Inheriting instead would hand the model the Medulla process's own
//! environment, which holds `MEDULLA_TOKEN`, router API keys and whatever else
//! the operator exported; `env` is a one-word command, so that is a direct
//! read of the host's credentials by anything the model decides to run.
//!
//! # What this does and does not claim
//!
//! This runs a command with the checkout as its working directory, a curated
//! environment, and a deadline. It is **not** a sandbox: a command can still
//! reach anything on the filesystem the Medulla process can, and `cwd` bounds
//! where relative paths resolve, not what the process may touch. The filesystem tools beside it enforce containment
//! because they own the path; a shell command's reach is the operating system's
//! to decide, and pretending otherwise in a doc comment is how an operator ends
//! up trusting a boundary that was never there.
//!
//! That is a deliberate scope, not an oversight — the embedded core this
//! replaced put a security policy, an approval gate and an optional OS jail in
//! front of the same call. Rebuilding that belongs with whoever re-adds
//! unattended execution; until then the tool is honest about being a plain
//! spawn, and [`ShellTool::new`] takes the deadline rather than defaulting it.

use std::process::Stdio;
use std::sync::Arc;

use tokio::io::AsyncReadExt;

use serde_json::{json, Value};
use tinyagents::error::{Result as TaResult, TinyAgentsError};
use tinyagents::tool::{Tool, ToolResult};
use tinyinference::tool::{ToolCall, ToolFormat, ToolSchema};

use super::fs::Workspace;

/// Run a command in the checkout.
pub struct ShellTool {
    workspace: Workspace,
    timeout: std::time::Duration,
    /// The complete environment the command runs under. Not merged with this
    /// process's own — it replaces it.
    env: std::collections::HashMap<String, String>,
}

impl ShellTool {
    /// Root a shell at `workspace`, with `timeout` as the per-command deadline
    /// and `env` as the command's entire environment.
    pub fn new(
        workspace: Workspace,
        timeout: std::time::Duration,
        env: std::collections::HashMap<String, String>,
    ) -> Self {
        Self {
            workspace,
            timeout,
            env,
        }
    }

    /// The interpreter this platform runs a command line through.
    fn interpreter() -> (&'static str, &'static str) {
        if cfg!(windows) {
            ("cmd", "/C")
        } else {
            ("sh", "-c")
        }
    }
}

/// Most output bytes kept per stream.
///
/// A build log is easily megabytes and every byte of it is model context; the
/// tail is kept rather than the head because the useful part of a failed command
/// is almost always at the end.
const MAX_OUTPUT_BYTES: usize = 16 * 1024;

/// Owns a spawned child and kills its whole process group when dropped.
///
/// `kill_on_drop` alone is not enough once the child leads a process group: it
/// reaps the direct child — the interpreter — and leaves the group behind. The
/// timeout path can call [`terminate_group`] explicitly, but an *outer*
/// cancellation cannot: aborting a turn drops this future wherever it happens to
/// be suspended, and there is no branch to run. A `Drop` impl is the only hook
/// that covers both, so the timeout path and an operator's abort clean up
/// identically.
struct GroupGuard {
    child: tokio::process::Child,
    /// The leader's pid, captured at spawn.
    ///
    /// Not read from the child at kill time. `Child::id()` answers `None` once
    /// the child has been reaped, and the case that matters is exactly that: a
    /// command like `long-build &` backgrounds a descendant that inherits the
    /// pipes, the shell leader exits, `wait()` reaps it — and the drain futures
    /// stay open on the descendant that is still running. Asking the reaped
    /// child for its pid then yields nothing to signal, so the group survives
    /// while the tool reports having killed it.
    ///
    /// The pid remains valid as a *group* id for as long as any member lives,
    /// which is precisely the window this needs.
    leader: Option<u32>,
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        terminate_group(self.leader, &mut self.child);
    }
}

/// Kill a command and everything it started.
///
/// On unix the child leads its own process group (see the spawn above), so
/// signalling the negated pid reaches every descendant. Failures are ignored:
/// the group is already gone if the child exited before this ran, which is the
/// common case on a command that simply finished.
///
/// On Windows there is no equivalent one-liner — killing a job object would mean
/// creating one at spawn — so this falls back to `start_kill`, which reaps the
/// interpreter and leaves its descendants. Naming the gap rather than implying
/// the platforms behave alike.
fn terminate_group(leader: Option<u32>, child: &mut tokio::process::Child) {
    #[cfg(unix)]
    {
        if let Some(pid) = leader {
            // SAFETY: `kill(2)` with a negative pid signals a process group.
            // The pid was captured at spawn from a child this call owns; the
            // group outlives the reaped leader, and the id cannot be recycled
            // onto an unrelated group while any member of this one is alive.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
    let _ = child.start_kill();
}

/// A bounded tail of one output stream, read as the process produces it.
///
/// # Why this is not `wait_with_output`
///
/// That call buffers the whole stream in memory and hands it back at exit, so
/// the cap could only ever be applied *after* the fact — and a command like
/// `yes` produces output faster than any deadline expires. The process would
/// take the host down long before the truncation it was supposedly subject to
/// ran. Reading into a ring here means an unbounded command costs a fixed
/// [`MAX_OUTPUT_BYTES`] regardless of how much it writes.
///
/// Draining rather than stopping at the cap is deliberate: closing the pipe
/// would hand the child a `SIGPIPE`/`EPIPE` on its next write, killing a command
/// for being chatty rather than for failing. It keeps writing into a buffer that
/// discards the oldest bytes.
struct BoundedTail {
    /// The most recent bytes, oldest dropped first.
    buf: std::collections::VecDeque<u8>,
    /// Bytes discarded to stay under the cap, reported to the model.
    dropped: usize,
}

impl BoundedTail {
    fn new() -> Self {
        Self {
            buf: std::collections::VecDeque::with_capacity(MAX_OUTPUT_BYTES),
            dropped: 0,
        }
    }

    fn extend(&mut self, chunk: &[u8]) {
        for &byte in chunk {
            if self.buf.len() == MAX_OUTPUT_BYTES {
                self.buf.pop_front();
                self.dropped += 1;
            }
            self.buf.push_back(byte);
        }
    }

    /// The kept bytes as text, prefixed with what was dropped.
    fn finish(self) -> String {
        let dropped = self.dropped;
        let bytes: Vec<u8> = self.buf.into();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        if dropped == 0 {
            return text;
        }
        format!("[truncated: {dropped} earlier bytes omitted]\n{text}")
    }
}

/// Drain `reader` to end-of-stream, keeping only a bounded tail.
async fn drain<R>(mut reader: R) -> std::io::Result<String>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut tail = BoundedTail::new();
    let mut chunk = [0u8; 8192];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            return Ok(tail.finish());
        }
        tail.extend(&chunk[..read]);
    }
}

#[async_trait::async_trait]
impl Tool<()> for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Run a shell command with the workspace as its working directory. \
         Returns the exit status with captured stdout and stderr."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The command line to run.",
                    },
                },
                "required": ["command"],
            }),
            format: ToolFormat::default(),
        }
    }

    async fn call(&self, _state: &(), call: ToolCall) -> TaResult<ToolResult> {
        let command = call
            .arguments
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                TinyAgentsError::Tool("`command` is required and must be a string".to_string())
            })?
            .to_string();

        let (shell, flag) = Self::interpreter();
        let started = std::time::Instant::now();
        let mut spawner = tokio::process::Command::new(shell);
        spawner
            .arg(flag)
            .arg(&command)
            .current_dir(&self.workspace.root)
            .env_clear()
            .envs(&self.env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Put the child in its own process group so the timeout can signal the
        // whole tree. `kill_on_drop` reaps the direct child only — the `sh` — and
        // a model that ran `cargo build` or `npm test` leaves the real worker
        // orphaned, still holding the checkout and still burning the machine
        // long after the tool reported a timeout.
        #[cfg(unix)]
        spawner.process_group(0);
        let spawned = spawner
            .spawn()
            .map_err(|e| TinyAgentsError::Tool(format!("could not start `{shell}`: {e}")))?;
        // Captured now, while the child is certainly unreaped.
        let leader = spawned.id();
        let mut guard = GroupGuard {
            child: spawned,
            leader,
        };
        let child = &mut guard.child;

        // Taken before the wait so both pipes can be drained concurrently with
        // it. Draining is not optional: a child that fills its stdout pipe
        // blocks forever on the write, so a wait that is not reading is a
        // deadlock for any command that outproduces the pipe buffer.
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();
        let pump = async {
            let out = async {
                match stdout_pipe {
                    Some(pipe) => drain(pipe).await,
                    None => Ok(String::new()),
                }
            };
            let err = async {
                match stderr_pipe {
                    Some(pipe) => drain(pipe).await,
                    None => Ok(String::new()),
                }
            };
            let (out, err, status) = tokio::join!(out, err, child.wait());
            Ok::<_, std::io::Error>((out?, err?, status?))
        };

        // `kill_on_drop` above is what makes the timeout real: without it the
        // future is dropped on elapse and the process keeps running, holding
        // the checkout and its pipes open with nothing left to read them.
        let (stdout, stderr, status) = match tokio::time::timeout(self.timeout, pump).await {
            Ok(Ok(triple)) => triple,
            Ok(Err(e)) => return Err(TinyAgentsError::Tool(format!("`{command}` failed: {e}"))),
            Err(_) => {
                return Ok(ToolResult {
                    call_id: call.id,
                    name: self.name().to_string(),
                    content: format!(
                        "command timed out after {}s and was killed",
                        self.timeout.as_secs()
                    ),
                    raw: None,
                    // Reported as a tool error, not a run failure: a timeout is
                    // information the model can act on (narrow the command, add
                    // a filter) and ending the turn over it throws that away.
                    error: Some("timeout".to_string()),
                    elapsed_ms: started.elapsed().as_millis() as u64,
                });
            }
        };

        let code = status.code();
        let mut content = format!(
            "exit: {}\n",
            code.map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string())
        );
        if !stdout.trim().is_empty() {
            content.push_str(&format!("--- stdout ---\n{stdout}\n"));
        }
        if !stderr.trim().is_empty() {
            content.push_str(&format!("--- stderr ---\n{stderr}\n"));
        }
        if stdout.trim().is_empty() && stderr.trim().is_empty() {
            content.push_str("(no output)\n");
        }

        Ok(ToolResult {
            call_id: call.id,
            name: self.name().to_string(),
            content,
            raw: Some(json!({ "exitCode": code })),
            // A non-zero exit is a *result*, not a tool failure: the model asked
            // to run a command and it ran. Reporting it as an error would end
            // the run on the first failing test, which is usually the moment the
            // turn is most useful.
            error: None,
            elapsed_ms: started.elapsed().as_millis() as u64,
        })
    }
}

/// The shell tool, rooted at one checkout.
pub fn all(
    workspace: Workspace,
    timeout: std::time::Duration,
    env: std::collections::HashMap<String, String>,
) -> Vec<Arc<dyn Tool<()>>> {
    vec![Arc::new(ShellTool::new(workspace, timeout, env))]
}

#[cfg(test)]
mod tests;
