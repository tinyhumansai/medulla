//! Provider discovery and invocation shaping: which daemon providers exist, how
//! to resolve a provider's binary, whether a provider accepts mid-run stdin, and
//! how to build the one-shot headless argv for each provider.

use std::collections::HashMap;

use crate::protocol::HarnessProvider;

use super::types::ExistsOnPath;

/// Every daemon-supported provider.
pub const DAEMON_PROVIDERS: [HarnessProvider; 3] = [
    HarnessProvider::Claude,
    HarnessProvider::Codex,
    HarnessProvider::Opencode,
];

/// Resolve the binary name/path for a provider (env override wins). Delegates to
/// the central resolver ([`crate::protocol::env::provider_bin`]) so the
/// daemon and wrapper share one bin-override contract.
pub fn provider_bin(provider: HarnessProvider, env: &HashMap<String, String>) -> String {
    crate::protocol::env::provider_bin(provider, env)
}

/// Default lookup: a path-ish name is probed directly for `X_OK`, a bare name is
/// searched across `$PATH` entries.
pub fn make_path_lookup(env: &HashMap<String, String>) -> ExistsOnPath {
    let path = env.get("PATH").cloned().unwrap_or_default();
    let dirs: Vec<String> = path
        .split(path_separator())
        .filter(|d| !d.is_empty())
        .map(str::to_string)
        .collect();
    Box::new(move |bin: &str| {
        if bin.contains('/') || bin.contains('\\') {
            return is_executable_command(std::path::Path::new(bin));
        }
        dirs.iter()
            .any(|dir| is_executable_command(&std::path::Path::new(dir).join(bin)))
    })
}

#[cfg(windows)]
/// Mirror `Command::new` resolution: retain configured extensions and add only
/// `.exe` to extensionless names, never advertising PATHEXT script shims that
/// the direct process spawn cannot execute.
fn is_executable_command(path: &std::path::Path) -> bool {
    if path.extension().is_some() {
        return is_executable(path);
    }
    let mut candidate = path.as_os_str().to_os_string();
    candidate.push(".exe");
    is_executable(std::path::Path::new(&candidate))
}

#[cfg(not(windows))]
fn is_executable_command(path: &std::path::Path) -> bool {
    is_executable(path)
}

#[cfg(windows)]
fn path_separator() -> char {
    ';'
}
#[cfg(not(windows))]
fn path_separator() -> char {
    ':'
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() && meta.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
}

/// Which of the (optionally restricted) providers have a binary on PATH.
///
/// OpenHuman is deliberately not among them, and not because it cannot run —
/// it can, in this very process. It is absent because this list answers a
/// narrower question than "what could serve a task": it answers *does this
/// machine have a coding agent installed*, and that is what decides whether a
/// host starts at all and which provider an unrouted task falls back to. A
/// machine with no CLI must still refuse to host rather than accept every
/// delegated coding task and answer it with an OpenHuman chat turn.
///
/// So OpenHuman is reachable by being *named* — see
/// [`HarnessProvider::needs_binary`] and the dispatch in
/// [`crate::daemon::task_loop`] — never by implicit fallback. An operator who
/// *explicitly* configures `openhuman` as the host's default has named it, so
/// the embedded host accepts and honours that default; see
/// [`crate::daemon::embedded::EmbeddedDaemon::start`].
pub fn detect_providers(
    env: &HashMap<String, String>,
    only: Option<&[HarnessProvider]>,
    exists_on_path: Option<&ExistsOnPath>,
) -> Vec<HarnessProvider> {
    let owned_lookup;
    let lookup: &ExistsOnPath = match exists_on_path {
        Some(lookup) => lookup,
        None => {
            owned_lookup = make_path_lookup(env);
            &owned_lookup
        }
    };
    let candidates: &[HarnessProvider] = only.unwrap_or(&DAEMON_PROVIDERS);
    candidates
        .iter()
        .copied()
        .filter(|provider| lookup(&provider_bin(*provider, env)))
        .collect()
}

/// Build the argv for a one-shot headless run of `provider`, starting a fresh
/// session.
///
/// Thin wrapper over `build_resumed_run_args` with no session to resume.
pub fn build_run_args(
    provider: HarnessProvider,
    prompt: &str,
    model: Option<&str>,
    agent: Option<&str>,
    extra_args: &[String],
    skip_permissions: bool,
) -> Vec<String> {
    build_resumed_run_args(
        provider,
        prompt,
        model,
        agent,
        extra_args,
        skip_permissions,
        None,
    )
}

/// Build the argv for a one-shot headless run of `provider`, optionally
/// resuming a previously captured harness session.
///
/// `resume` is ignored for providers that cannot resume, so passing one is
/// always safe. Note the shape difference the two CLIs impose: claude takes
/// `--resume <id>` as a flag anywhere in the argv, while codex takes `resume
/// <id>` as a **subcommand** that must directly follow `exec` — which is why
/// resume cannot simply be passed through `extra_args`.
#[allow(clippy::too_many_arguments)]
pub fn build_resumed_run_args(
    provider: HarnessProvider,
    prompt: &str,
    model: Option<&str>,
    agent: Option<&str>,
    extra_args: &[String],
    skip_permissions: bool,
    resume: Option<&str>,
) -> Vec<String> {
    // A prompt beginning with "-" would be parsed as a flag by the provider (an
    // injection vector since task text is remote-controlled); neutralize with a
    // leading space, which the model sees as insignificant.
    let prompt = if prompt.starts_with('-') {
        format!(" {prompt}")
    } else {
        prompt.to_string()
    };
    let resume = resume.filter(|id| !id.trim().is_empty() && can_resume(provider));
    let mut args: Vec<String> = Vec::new();
    match provider {
        HarnessProvider::Claude => {
            args.extend(["-p", "--output-format", "stream-json", "--verbose"].map(String::from));
            if let Some(resume) = resume {
                args.push("--resume".to_string());
                args.push(resume.to_string());
            }
            if skip_permissions {
                args.push("--dangerously-skip-permissions".to_string());
            }
            // Claude Code takes the session model via the long `--model` flag
            // (codex/opencode use `-m`).
            if let Some(model) = model {
                args.push("--model".to_string());
                args.push(model.to_string());
            }
            args.extend(extra_args.iter().cloned());
            // End option parsing before the prompt. Claude Code has variadic
            // options - `--add-dir <directories...>` is one, and it is what
            // `workflows::skills::spawn_args` appends so a session can see its
            // managed skills - and a variadic sitting last in `extra_args`
            // swallows the prompt as one more value. The harness then exits 1
            // with "Input must be provided either through stdin or as a prompt
            // argument", which reads like a Medulla bug in building the prompt
            // rather than one in where it was placed.
            //
            // `--` is the fix rather than an ordering rule, because ordering
            // only holds while the last extra arg happens to be bounded: the
            // next variadic option added anywhere reintroduces this silently.
            args.push("--".to_string());
            args.push(prompt);
        }
        HarnessProvider::Codex => {
            args.push("exec".to_string());
            // `resume` is a subcommand and must sit directly after `exec`.
            if let Some(resume) = resume {
                args.push("resume".to_string());
                args.push(resume.to_string());
            }
            args.push("--json".to_string());
            // Codex defaults to `workspace-write`: writable root is the cwd and
            // the network is off. A daemon task cannot live inside that. The
            // git directory of a linked worktree is *outside* the worktree -
            // for a submodule checkout it is several levels up, under the
            // superproject's `.git/modules/<name>/worktrees/<id>` - so `git
            // commit` cannot write its refs, and with no DNS `git push` and
            // `gh` fail outright. A task that can edit and verify but never
            // land the result is worse than one that refuses to start.
            //
            // Gated on the same operator setting that gives Claude
            // `--dangerously-skip-permissions` above: one consent flag, honoured
            // by every harness, rather than a per-provider surprise.
            if skip_permissions {
                args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
            }
            if let Some(model) = model {
                args.push("-m".to_string());
                args.push(model.to_string());
            }
            args.extend(extra_args.iter().cloned());
            args.push(prompt);
        }
        HarnessProvider::Opencode => {
            args.push("run".to_string());
            if let Some(model) = model {
                args.push("-m".to_string());
                args.push(model.to_string());
            }
            args.push("--agent".to_string());
            args.push(agent.unwrap_or("build").to_string());
            args.push("--format".to_string());
            args.push("json".to_string());
            args.extend(extra_args.iter().cloned());
            args.push(prompt);
        }
        // Operator-only; daemon parsing rejects it before reaching this seam.
        HarnessProvider::Openhuman => args.push("tui".to_string()),
        // Never dispatched to at all — `dispatchable_from_wire` refuses the
        // name — so there is no headless invocation to build.
        HarnessProvider::Shell => {}
    }
    args
}

/// Whether a provider can resume a previously captured harness session id.
///
/// Re-exported from [`crate::sessions::routing`] so the argv builder can gate on
/// it without the providers module depending on the sessions module.
fn can_resume(provider: HarnessProvider) -> bool {
    matches!(provider, HarnessProvider::Claude | HarnessProvider::Codex)
}

/// The stream field a provider announces its own session id on.
///
/// claude stamps `session_id` on **every** frame (the `system`/`init` one is
/// merely the first); codex announces `thread_id` on `thread.started`.
/// `opencode` announces nothing, so it has no session to capture.
pub fn session_id_field(provider: HarnessProvider) -> Option<&'static str> {
    match provider {
        HarnessProvider::Claude => Some("session_id"),
        HarnessProvider::Codex => Some("thread_id"),
        HarnessProvider::Opencode | HarnessProvider::Openhuman => None,
        // No stream to read a session id off: a shell writes to a terminal.
        HarnessProvider::Shell => None,
    }
}

/// Extract a provider's own session id from one raw stream line.
///
/// Returns `None` for a non-JSON line, a provider that announces none, or a
/// blank value. Callers keep the **first** id they see: re-reading it on every
/// frame would be waste, and a run reports exactly one session.
pub fn extract_session_id(provider: HarnessProvider, raw: &str) -> Option<String> {
    let field = session_id_field(provider)?;
    let parsed: serde_json::Value = serde_json::from_str(raw).ok()?;
    parsed
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// Whether a provider accepts mid-run stdin input (`input` frames).
///
/// `opencode run` and `codex exec` both treat a non-TTY stdin as prompt content
/// and block at startup reading it until EOF; neither has an interactive mid-run
/// stdin channel. Piping (and holding) their stdin open therefore deadlocks the
/// run, so they get an immediate-EOF null stdin and `input` frames are rejected
/// up front. Only `claude -p` accepts forwarded `input` frames over a live stdin
/// pipe.
pub fn supports_stdin(provider: HarnessProvider) -> bool {
    matches!(provider, HarnessProvider::Claude)
}

/// The wire name for a provider.
pub fn provider_name(provider: HarnessProvider) -> &'static str {
    provider.as_str()
}
