//! Local execution of observation-only hooks for ACP Codex sessions.
//!
//! `codex app-server` executes no lifecycle hooks of its own, so this module is
//! the seam that runs an operator's `PostToolUse` hooks after each completed
//! ACP tool call. It is **observation-only**: ACP gives the client no way to
//! amend a tool call it did not execute, so a hook's stdout decision is never
//! forwarded into the session. That restriction is reported to the operator at
//! delivery time (see [`super::delivery`]).

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use regex::Regex;
use serde_json::{json, Map, Value};
use tokio::io::AsyncWriteExt;

use crate::harness_hooks::HookSpec;

/// Run matching `PostToolUse` hooks without allowing a hook failure to fail a turn.
///
/// For every hook whose matcher selects `tool`, this starts the hook's command
/// through the platform shell with `cwd` as its working directory, writes the
/// JSON payload to the command's stdin, and waits for it to exit. A hook that
/// fails to start, exits non-zero, times out, or cannot be waited on is logged
/// and skipped — never returned — so a broken hook cannot fail the ACP turn it
/// observes.
///
/// `env` is the per-session environment already seeded with the hook grant (see
/// [`crate::harness_hooks::seed_hook_grant`]), so a hook that reports back to a
/// control plane finds the socket and credential every direct spawn door seeds.
pub async fn run_post_tool_use(
    hooks: &[HookSpec],
    cwd: &Path,
    env: &HashMap<String, String>,
    session_id: &str,
    tool: &str,
    input: &Value,
) {
    for hook in hooks
        .iter()
        .filter(|hook| matches_tool(&hook.matcher, tool))
    {
        let payload = json!({
            "hook_event_name": "PostToolUse",
            "session_id": session_id,
            "cwd": cwd,
            "tool_name": tool,
            "tool_input": normalized_input(input),
        });
        let timeout = Duration::from_secs(hook.timeout().unwrap_or(600));
        let mut child = match spawn_hook(hook.command(), cwd, env) {
            Ok(child) => child,
            Err(error) => {
                tracing::warn!(
                    command = hook.command(),
                    "could not start ACP PostToolUse hook: {error}"
                );
                continue;
            }
        };
        // The payload write and the wait share one timeout. A hook that stays
        // alive without draining its stdin would otherwise let `write_all`
        // block forever on a payload larger than the OS pipe buffer, before
        // the wait timeout had even begun.
        let outcome = tokio::time::timeout(timeout, async {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(payload.to_string().as_bytes()).await;
            }
            child.wait().await
        })
        .await;
        match outcome {
            Ok(Ok(status)) if status.success() => {}
            Ok(Ok(status)) => tracing::warn!(
                command = hook.command(),
                ?status,
                "ACP PostToolUse hook failed"
            ),
            Ok(Err(error)) => tracing::warn!(
                command = hook.command(),
                "could not wait for ACP PostToolUse hook: {error}"
            ),
            Err(_) => {
                // The timeout expired mid-write or mid-wait, which does not
                // terminate the child. Kill it (with its whole process group,
                // since it runs under a shell that may have started
                // descendants) and reap.
                kill_hook(&mut child);
                let _ = child.wait().await;
                tracing::warn!(command = hook.command(), "ACP PostToolUse hook timed out");
            }
        }
    }
}

/// Spawn one hook through the platform shell, as its own process-group leader
/// on Unix so a timed-out hook can be terminated together with any descendants
/// it started.
fn spawn_hook(
    command: &str,
    cwd: &Path,
    env: &HashMap<String, String>,
) -> std::io::Result<tokio::process::Child> {
    let mut builder = tokio::process::Command::new(shell());
    builder
        .args(shell_args(command))
        .current_dir(cwd)
        .envs(env)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    builder.process_group(0);
    builder.spawn()
}

#[cfg(unix)]
fn shell() -> &'static str {
    "sh"
}

#[cfg(unix)]
fn shell_args(command: &str) -> [&str; 2] {
    ["-c", command]
}

#[cfg(windows)]
fn shell() -> &'static str {
    "cmd.exe"
}

#[cfg(windows)]
fn shell_args(command: &str) -> [&str; 4] {
    ["/D", "/S", "/C", command]
}

/// Terminate a timed-out hook and, on Unix, everything in its process group.
///
/// The hook was spawned through a shell that may have started descendants of
/// its own; killing only the shell would strand them. With `process_group(0)`
/// the shell leads its own group, so killing the negative pid reaches the whole
/// tree. On Windows the shell process itself is terminated, which is the best
/// `cmd.exe` offers without a Job Object.
fn kill_hook(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // The child leads its own process group (`process_group(0)` above), so
        // the negative pid is that group. Guard on a real pid: `kill(-0, …)`
        // would signal this process's own group.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
}

/// Whether `matcher` selects a tool whose ACP `kind` is `tool`.
///
/// An empty matcher and `*` both match every tool, matching the harnesses'
/// own `*` default. Any other matcher is a regex applied to the tool. The
/// matcher is written in the native Codex hook vocabulary (`Bash`,
/// `Edit|Write`, …), because the same declaration feeds the direct spawn door,
/// while ACP reports the semantic `ToolKind` (`execute`, `edit`, …). Matching is
/// therefore tried against the kind *and* its native aliases, so both
/// vocabularies work.
pub(super) fn matches_tool(matcher: &str, tool: &str) -> bool {
    matcher.is_empty()
        || matcher == "*"
        || tool_matcher_candidates(tool)
            .iter()
            .any(|candidate| Regex::new(matcher).is_ok_and(|regex| regex.is_match(candidate)))
}

/// The ACP `ToolKind` and the native Codex tool names it corresponds to.
///
/// ACP reports one category per call (`execute`, `edit`, …); Codex's own hooks
/// match on the concrete tool (`Bash`, `Edit|Write`, …). Keeping the raw kind
/// as a candidate lets an ACP-aware matcher target it directly. Kinds with no
/// native equivalent match only their own name.
pub(super) fn tool_matcher_candidates(kind: &str) -> Vec<String> {
    let mut candidates = vec![kind.to_string()];
    match kind {
        "execute" => candidates.extend(["Bash", "Shell"].map(str::to_string)),
        "edit" => candidates.extend(["Edit", "Write"].map(str::to_string)),
        "read" => candidates.extend(["Read", "View"].map(str::to_string)),
        "search" => candidates.extend(["Grep", "Glob"].map(str::to_string)),
        "fetch" => candidates.extend(["WebFetch"].map(str::to_string)),
        _ => {}
    }
    candidates
}

/// Fold an ACP `path`/`filePath` input key into the `file_path` spelling the
/// shared hook document and Codex's own hooks read, so a hook written for a
/// directly-spawned Codex session sees the same key over ACP.
///
/// A present `file_path` is preserved; otherwise the first of `path`/`filePath`
/// supplies it. Non-object inputs pass through unchanged.
pub(super) fn normalized_input(input: &Value) -> Value {
    let Value::Object(input) = input else {
        return input.clone();
    };
    let mut normalized: Map<String, Value> = input.clone();
    if !normalized.contains_key("file_path") {
        if let Some(path) = normalized
            .get("path")
            .or_else(|| normalized.get("filePath"))
            .cloned()
        {
            normalized.insert("file_path".to_string(), path);
        }
    }
    Value::Object(normalized)
}
