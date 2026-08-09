//! The in-process adapter that translates Medulla's operator hook config into
//! OpenHuman's embedder lifecycle hooks.
//!
//! [`super::boot_with_hooks`] asks this module to register the configured
//! `PreToolUse`, `PostToolUse`, and `Stop` hooks once, before the core builds.
//! Registration is process-global. OpenHuman's replacement API ensures a retry
//! or later boot replaces Medulla's hooks rather than retaining stale commands.
//!
//! Each [`HookSpec`] is kept whole rather than reduced to a command string so
//! two properties the operator declared survive translation: the **matcher**,
//! which scopes a tool hook to the tools it names instead of firing on every
//! one, and the **timeout**, which bounds a slow command so a stuck hook cannot
//! stall the tool call or the turn.
//!
//! Registration being process-global while the working directory is per-run is
//! the reason [`super::turn_cwd`] exists: the tool payload's `cwd` is read from
//! that task-local so a hook is told the directory the *turn* is working in.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use openhuman_core::openhuman::agent::hooks::{
    replace_embedder_post_turn_hook, replace_embedder_tool_hook, PostTurnHook, ToolHook,
    ToolHookContext, TurnContext,
};

use crate::harness_hooks::{HookEvent, HookSpec, HooksConfig};
use crate::protocol::HarnessProvider;

/// Register the configured OpenHuman lifecycle hooks in this process.
///
/// Called by [`super::boot_with_hooks`] before the core builds. Splits the
/// hooks that apply to OpenHuman by event and registers a stop hook and/or a
/// tool hook, each only when the config actually declares one — a host with no
/// hooks registers nothing.
///
/// Registration replaces the prior Medulla hook of each kind. Empty hook sets
/// remove their previous registration, so later boots cannot retain stale work.
pub(crate) fn register_lifecycle_hooks(hooks: &HooksConfig) {
    let mut stop = Vec::new();
    let mut pre = Vec::new();
    let mut post = Vec::new();
    for spec in hooks.for_provider(HarnessProvider::Openhuman) {
        match spec.event {
            HookEvent::Stop => stop.push(spec.clone()),
            HookEvent::PreToolUse => pre.push(spec.clone()),
            HookEvent::PostToolUse => post.push(spec.clone()),
            _ => {}
        }
    }
    replace_embedder_post_turn_hook(
        "medulla-stop-hook",
        (!stop.is_empty()).then(|| {
            std::sync::Arc::new(MedullaStopHook::new(stop)) as std::sync::Arc<dyn PostTurnHook>
        }),
    );
    replace_embedder_tool_hook(
        "medulla-tool-hook",
        (!pre.is_empty() || !post.is_empty()).then(|| {
            std::sync::Arc::new(MedullaToolHook::new(pre, post)) as std::sync::Arc<dyn ToolHook>
        }),
    );
}

/// Runs Medulla's command hooks when an in-process OpenHuman turn completes.
///
/// Exit status is deliberately ignored: a `Stop` hook observes, and a cleanup
/// command returning non-zero must not fail the turn. Each command is still
/// bounded by its configured timeout so a stuck one cannot stall the lifecycle.
struct MedullaStopHook {
    commands: Vec<HookSpec>,
    environment: HookEnvironment,
}

impl MedullaStopHook {
    /// Create a stop hook and retain any reporting grant its built-ins need.
    fn new(commands: Vec<HookSpec>) -> Self {
        let environment = HookEnvironment::for_specs(&commands, &[]);
        Self {
            commands,
            environment,
        }
    }
}

#[async_trait::async_trait]
impl PostTurnHook for MedullaStopHook {
    fn name(&self) -> &str {
        "medulla-stop-hook"
    }

    async fn on_turn_complete(&self, turn: &TurnContext) -> anyhow::Result<()> {
        let payload = serde_json::json!({
            "hook_event_name": "Stop", "session_id": turn.session_id,
            "agent_id": turn.agent_id, "prompt": turn.user_message,
            "response": turn.assistant_response,
        })
        .to_string();
        for spec in &self.commands {
            run_command(spec, payload.as_bytes(), false, &self.environment.values).await?;
        }
        Ok(())
    }
}

/// Medulla's synchronous/pre and observational/post tool hook adapter.
///
/// A `PreToolUse` failure (a non-zero exit or a timeout) vetoes the tool call by
/// returning an error. `PostToolUse` is observational: its commands still run,
/// but OpenHuman's dispatcher logs any error without changing the tool's result.
/// Both run only the hooks whose matcher selects the current tool name.
pub(super) struct MedullaToolHook {
    pre_commands: Vec<HookSpec>,
    post_commands: Vec<HookSpec>,
    environment: HookEnvironment,
}

impl MedullaToolHook {
    /// Create a tool hook and retain any reporting grant its built-ins need.
    pub(super) fn new(pre_commands: Vec<HookSpec>, post_commands: Vec<HookSpec>) -> Self {
        let environment = HookEnvironment::for_specs(&pre_commands, &post_commands);
        Self {
            pre_commands,
            post_commands,
            environment,
        }
    }
}

#[async_trait::async_trait]
impl ToolHook for MedullaToolHook {
    fn name(&self) -> &str {
        "medulla-tool-hook"
    }

    async fn before_tool(&self, context: &ToolHookContext) -> anyhow::Result<()> {
        run_hook_commands(&self.pre_commands, context, false, &self.environment.values).await
    }

    async fn after_tool(&self, context: &ToolHookContext) -> anyhow::Result<()> {
        // Post hooks are observational; errors are logged by OpenHuman rather
        // than retroactively turning a successful tool into a failure.
        run_hook_commands(&self.post_commands, context, true, &self.environment.values).await
    }
}

/// Run `specs` against a tool lifecycle with structured JSON on stdin.
///
/// Only hooks whose matcher selects `context.tool_name` run. A non-zero exit or
/// timeout is returned to the caller — for a pre-hook that vetoes the tool call,
/// for a post-hook OpenHuman logs it without changing the result.
pub(super) async fn run_hook_commands(
    specs: &[HookSpec],
    context: &ToolHookContext,
    continue_after_error: bool,
    environment: &HashMap<String, String>,
) -> anyhow::Result<()> {
    let payload = serde_json::to_vec(&tool_hook_payload(context))?;
    let mut first_error = None;
    for spec in specs {
        if !matcher_selects(&spec.matcher, &context.tool_name) {
            continue;
        }
        if let Err(error) = run_command(spec, &payload, true, environment).await {
            if !continue_after_error {
                return Err(error);
            }
            tracing::warn!(
                "[core_host] post-tool hook failed; running remaining matching hooks: {error:#}"
            );
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// Convert OpenHuman's typed callback into the command-hook envelope used by
/// Medulla's native harnesses.
///
/// `cwd` names the directory **this turn** is working in, taken from
/// [`super::turn_cwd`], because a `PostToolUse` auto-commit hook resolves its
/// target repository from it — OpenHuman's tool arguments carry neither a
/// `file_path` nor a `cd`, so `cwd` is the only signal it has. Reporting the
/// process's own directory here, as this adapter first did, made such a hook
/// commit whatever repository Medulla was launched in rather than the run's
/// workspace.
///
/// Falls back to the process directory only when no run declared one — a TUI
/// chat turn or an MCP call, where the process directory *is* the answer.
pub(super) fn tool_hook_payload(context: &ToolHookContext) -> serde_json::Value {
    let cwd = super::turn_cwd::current_turn_cwd()
        .or_else(|| std::env::current_dir().ok())
        .and_then(|path| path.to_str().map(str::to_owned));
    serde_json::json!({
        "hook_event_name": match context.event {
            openhuman_core::openhuman::agent::hooks::ToolHookEvent::PreToolUse => "PreToolUse",
            openhuman_core::openhuman::agent::hooks::ToolHookEvent::PostToolUse => "PostToolUse",
        },
        "session_id": context.call_id,
        "cwd": cwd,
        "tool_name": context.tool_name,
        "tool_input": context.arguments,
        "success": context.success,
        "duration_ms": context.duration_ms,
    })
}

/// Run one hook `spec` with `payload` on stdin, bounded by its configured
/// timeout.
///
/// `enforce_status` controls whether a non-zero exit is an error: pre-hooks veto
/// on failure, while stop hooks observe. Every command is bounded by the hook's
/// configured timeout (or this adapter's finite default when none is declared); a command
/// that exceeds it is killed so a stuck hook cannot stall the tool call or the
/// turn. When `enforce_status` is set, a timeout also vetoes, matching a
/// pre-hook's blocking semantics.
pub(super) async fn run_command(
    spec: &HookSpec,
    payload: &[u8],
    enforce_status: bool,
    environment: &HashMap<String, String>,
) -> anyhow::Result<()> {
    let mut process = shell_command(spec.command(), environment);
    let mut child = process.spawn()?;
    let timeout = Duration::from_secs(spec.timeout().unwrap_or(DEFAULT_HOOK_TIMEOUT_SECS));
    let waited = tokio::time::timeout(timeout, async {
        let write_result = if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(payload).await
        } else {
            Ok(())
        };
        let status = child.wait().await?;
        Ok::<_, std::io::Error>((status, write_result))
    })
    .await;
    match waited {
        Ok(Ok((status, write_result))) => {
            if enforce_status {
                write_result?;
                anyhow::ensure!(status.success(), "hook command exited with {status}");
            }
            Ok(())
        }
        Ok(Err(err)) => Err(err.into()),
        Err(_elapsed) => {
            terminate_hook_process(&mut child).await;
            tracing::warn!(
                "[core_host] hook timed out and was killed: {}",
                spec.command()
            );
            if enforce_status {
                anyhow::bail!("hook command timed out: {}", spec.command());
            }
            Ok(())
        }
    }
}

/// Finite bound used when an operator leaves a hook timeout unspecified.
const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 60;

/// Kill the shell and all descendants it started, then reap the direct child.
async fn terminate_hook_process(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // The child is a process-group leader (see `shell_command`), so negative
        // PID selects the group and stops the shell together with its command.
        unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
    }

    #[cfg(windows)]
    if let Some(pid) = child.id() {
        let _ = tokio::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status()
            .await;
    }

    let _ = child.kill().await;
    let _ = child.wait().await;
}

/// The directory a hook command should start in, when the running turn declared
/// one that still exists.
///
/// `None` outside a turn scope (a TUI chat turn, an MCP call, or the
/// `tokio::spawn`-ed post-turn hook), and for a declared directory that has
/// since been removed — in both cases the child inherits this process's
/// directory, which is what it did before this existed and is a directory that
/// is certain to be there. Spawning into a missing directory would instead fail
/// the hook outright.
pub(super) fn hook_working_dir() -> Option<std::path::PathBuf> {
    super::turn_cwd::current_turn_cwd().filter(|path| path.is_dir())
}

/// A `sh -c` / `cmd /C` child for `command`, with hook JSON piped in on stdin.
///
/// The child starts in [`hook_working_dir`], not in Medulla's own process
/// directory: a hook script that runs `git commit` without an explicit `-C`
/// resolves the repository from where it was started, so inheriting this
/// process's directory made it operate on whichever checkout Medulla was
/// launched from. The payload's `cwd` said the right thing while `$PWD` said
/// another — the two now agree.
fn shell_command(command: &str, environment: &HashMap<String, String>) -> tokio::process::Command {
    let mut process = tokio::process::Command::new(if cfg!(windows) { "cmd" } else { "sh" });
    if cfg!(windows) {
        process.args(["/C", command]);
    } else {
        process.args(["-c", command]);
    }
    if let Some(cwd) = hook_working_dir() {
        process.current_dir(cwd);
    }
    process
        .envs(environment)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        // A distinct group makes timeout cleanup include shell descendants.
        unsafe {
            process.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    process
}

/// Environment and lifetime guard for built-in reporting hooks.
///
/// An embedded core starts commands itself, unlike the other harness spawn
/// doors, so it must mint and retain the control-plane grant until OpenHuman
/// drops the callback. Operator commands inherit the process environment as
/// before; only built-ins need the extra values.
struct HookEnvironment {
    values: HashMap<String, String>,
    _grant: Option<crate::harness_hooks::HookGrantGuard>,
}

impl HookEnvironment {
    /// Seed one unique reporting grant when these specs include a built-in.
    fn for_specs(first: &[HookSpec], second: &[HookSpec]) -> Self {
        let mut values = HashMap::new();
        let has_builtin = first.iter().chain(second).any(|spec| spec.builtin);
        let grant = has_builtin.then(|| {
            let sequence = NEXT_HOOK_GRANT.fetch_add(1, Ordering::Relaxed);
            crate::harness_hooks::seed_hook_grant(
                format!("embedded-openhuman-{}-{sequence}", std::process::id()),
                &mut values,
            )
        });
        Self {
            values,
            _grant: grant,
        }
    }
}

/// Distinguishes grants retained by stop and tool callbacks in one process.
static NEXT_HOOK_GRANT: AtomicU64 = AtomicU64::new(0);

/// Whether a hook's matcher selects `tool_name`.
///
/// A matcher is one or more `|`-separated glob patterns (`Edit|Write`), with `*`
/// the match-all default. Each alternation is anchored and `*` matches any run
/// of characters — the selective matchers operators write and the wildcard
/// default, not shell super-wildcards.
pub(super) fn matcher_selects(matcher: &str, tool_name: &str) -> bool {
    matcher
        .split('|')
        .filter(|pattern| !pattern.is_empty())
        .any(|pattern| pattern_selects(pattern, tool_name))
}

/// Whether one `|`-separated glob pattern selects `tool_name`.
fn pattern_selects(pattern: &str, tool_name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let mut source = String::from("\\A");
    for ch in pattern.chars() {
        match ch {
            '*' => source.push_str(".*"),
            '?' => source.push('.'),
            ch => source.push_str(&regex::escape(&ch.to_string())),
        }
    }
    source.push_str("\\z");
    regex::Regex::new(&source)
        .map(|pattern| pattern.is_match(tool_name))
        .unwrap_or(false)
}
