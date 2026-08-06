//! Headless one-shot execution: spawn the provider CLI, stream its JSONL output
//! through the shared semantic-event mappers to derive status updates and the
//! final reply, enforce an idle watchdog + cooperative abort, and retry transient
//! opencode SQLite-lock exits with jittered exponential backoff.

use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::protocol::HarnessProvider;

use super::super::mappers::HarnessLineMapper;
use super::detect::{
    build_resumed_run_args, extract_session_id, provider_bin, provider_name, supports_stdin,
};
use super::types::{OnEvent, OnStdin, RunSpec, RunTaskOptions, RunTaskResult};

/// A record that never terminates in a newline is dropped past this size.
const MAX_RECORD_BYTES: usize = 1_048_576;
/// Cap on the retained stdout/stderr tail (bytes).
pub(super) const TAIL_CAP: usize = 8192;
/// Maximum transient-lock retry attempts.
const LOCK_RETRY_ATTEMPTS: u32 = 5;
/// Base backoff (ms) for the transient-lock retry.
const LOCK_RETRY_BASE_MS: u64 = 250;

/// opencode's SQLite session store throws this when runs start too close
/// together; transient, clears on a short retry.
pub fn is_transient_lock(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("database is locked")
        || lower.contains("database table is locked")
        || lower.contains("sqlite_busy")
}

/// Append an auth-diagnosis hint to an opaque provider server error.
pub fn with_auth_hint(message: &str) -> String {
    let lower = message.to_lowercase();
    let auth_shaped = lower.contains("unexpected server error")
        || lower.contains("unauthorized")
        || lower.contains("401")
        || lower.contains("api key")
        || lower.contains("credential");
    if auth_shaped {
        format!("{message} — hint: the provider may lack credentials (run `opencode auth login` or export the provider API key)")
    } else {
        message.to_string()
    }
}

/// Run one delegated task headlessly, retrying transient opencode SQLite-lock
/// exits with jittered exponential backoff.
pub async fn run_provider_task(mut options: RunTaskOptions) -> Result<RunTaskResult, String> {
    // Ahead of the ACP branch on purpose: both transports end up talking to the
    // same endpoint with the same credential, so both must be routed through
    // Medulla's loopback proxy for the attribution headers on the wire to be ours
    // and for the child to never hold the real key. A no-op for every endpoint
    // that is not OpenRouter.
    crate::inference_proxy::route_spawn(options.provider, &mut options.router, &mut options.env)?;
    // Ahead of ACP because the two are chosen by different questions and a task
    // may answer both: ACP is a *protocol* switch on the environment, while the
    // app-server is the flavor the operator named. Naming `codex-server` is the
    // more specific statement, so it wins.
    if super::codex_server::uses_app_server(&options) {
        return super::codex_server::run_codex_server_task(options).await;
    }
    if super::acp::uses_acp(&options) {
        // ACP dispatch cannot carry Medulla hooks. Every other path injects them
        // onto the harness CLI's own argv, but here Medulla spawns an ACP *server*
        // (`@agentclientprotocol/claude-agent-acp`, `codex-acp`, `opencode acp`)
        // which spawns the harness itself, so there is no argv to add to. Claude
        // Code's env-based settings paths were checked as an alternative and do
        // not deliver hooks either. Say so rather than let the operator believe a
        // configured hook is running.
        let configured = options.hooks.for_provider(options.provider).len();
        if configured > 0 {
            tracing::warn!(
                provider = options.provider.as_str(),
                hooks = configured,
                "medulla hooks are not installed for ACP dispatch: the harness CLI is \
                 launched by the ACP server, not by Medulla",
            );
        }
        return super::acp::run_acp_task(options).await;
    }
    let mut on_event = options.on_event;
    let mut on_stdin = options.on_stdin;
    let mut on_session = options.on_session;
    let router = options.router;
    let env = options.env;
    let spec = RunSpec {
        provider: options.provider,
        prompt: options.prompt,
        cwd: options.cwd,
        env,
        timeout_ms: options.timeout_ms,
        model: options.model,
        agent: options.agent,
        extra_args: options.extra_args,
        skip_permissions: options.skip_permissions,
        resume_session_id: options.resume_session_id,
        workspace_context: options.workspace_context,
        abort: options.abort,
        router,
        attribution: options.attribution,
        hooks: options.hooks,
        on_workspace_context: options.on_workspace_context,
    };
    let mut attempt: u32 = 1;
    loop {
        // Callbacks are single-use; the retry path (opencode lock) runs without
        // them, mirroring the rarity of that branch.
        let attempt_on_event = on_event.take();
        let attempt_on_stdin = on_stdin.take();
        let attempt_on_session = on_session.take();
        match run_provider_attempt(
            &spec,
            attempt_on_event,
            attempt_on_stdin,
            attempt_on_session,
        )
        .await
        {
            Ok(result) => return Ok(result),
            Err(message) => {
                if !is_transient_lock(&message)
                    || attempt >= LOCK_RETRY_ATTEMPTS
                    || spec.abort.is_aborted()
                {
                    return Err(message);
                }
                let jitter = 0.5 + rand_unit();
                let delay =
                    (LOCK_RETRY_BASE_MS as f64 * 2f64.powi((attempt - 1) as i32) * jitter) as u64;
                tokio::time::sleep(Duration::from_millis(delay)).await;
                attempt += 1;
            }
        }
    }
}

/// A cheap uniform-ish `[0,1)` sample (no `rand` dep): folds the wall clock.
pub(super) fn rand_unit() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos % 1_000_000) as f64 / 1_000_000.0
}

/// One spawn-and-stream attempt of a headless run (the retry loop wraps this).
async fn run_provider_attempt(
    spec: &RunSpec,
    mut on_event: Option<OnEvent>,
    on_stdin: Option<OnStdin>,
    mut on_session: Option<super::types::OnSession>,
) -> Result<RunTaskResult, String> {
    if spec.abort.is_aborted() {
        return Err(format!(
            "{} task aborted before start",
            provider_name(spec.provider)
        ));
    }

    // `TINYPLACE_<P>_ARGS` (whitespace-split) is prepended to any configured
    // extra args, so a per-provider env override applies to headless daemon runs
    // too — matching the wrapper's child-argv prefix.
    let mut extra_args = crate::protocol::env::provider_args(spec.provider, &spec.env);
    // Medulla-launched harnesses attribute their commits to Medulla via a
    // `Co-authored-by` trailer. Nothing is persisted — the flags live only on
    // this child's argv. Empty for providers with no such knob.
    // Attribution and the operator's Medulla hooks share Claude Code's single
    // `--settings` flag, so they are built together — see
    // `harness_hooks::launch_args`.
    let (launch_args, hook_notes) =
        crate::harness_hooks::launch_args(spec.provider, spec.attribution, &spec.hooks);
    extra_args.extend(launch_args);
    for note in &hook_notes {
        tracing::warn!(provider = spec.provider.as_str(), "{note}");
    }
    // For providers that use the git-hook path (Codex, Opencode), merge the
    // prepare-commit-msg hook env vars into the child's environment.
    let mut merged_env = spec.env.clone();
    let attribution_env = crate::attribution::attribution_env(spec.attribution, &merged_env);
    merged_env.extend(attribution_env);
    // The built-in reporting hooks just installed onto `extra_args` need this
    // to find anything to report to — without it they spawn, find no grant,
    // and exit, on every one of `PostToolUse`'s per-tool-call firings for the
    // life of this run. Kept alive for the rest of this function (dropped at
    // the end of its scope, after the child has fully exited), so the grant
    // is revoked the moment this headless run is done rather than left live
    // for the rest of the process. Unique per run: two runs sharing a session
    // key would share a grant.
    let hook_grant_session = format!("headless-{}", uuid::Uuid::new_v4());
    let _hook_grant = crate::harness_hooks::seed_hook_grant(hook_grant_session, &mut merged_env);
    // Custom OpenAI-compatible router: layer the provider's endpoint env (and,
    // when configured, its API key) into the child at the spawn seam, so headless
    // daemon, operator-TUI daemon, and interactive wrappers all route identically.
    // The endpoint is a literal; the API key is resolved HERE, at spawn, from this
    // run's own environment BY NAME — it is never inlined into config, and it never
    // reaches a task frame, a status detail, or a log line. A configured key whose
    // env var is absent is a hard error (an explicit error frame upstream), never a
    // silent empty key that would spawn the harness unauthenticated.
    if let Some(router) = &spec.router {
        let injection = crate::protocol::env::router_env(spec.provider, router);
        for (key, value) in injection.env {
            merged_env.insert(key, value);
        }
        for (child_var, source_name) in injection.secret_env {
            match spec.env.get(&source_name).filter(|v| !v.is_empty()) {
                Some(secret) => {
                    merged_env.insert(child_var, secret.clone());
                }
                None => {
                    return Err(format!(
                        "router API key env var `{source_name}` is not set; \
                         export it or remove apiKeyEnv from [router]"
                    ));
                }
            }
        }
        extra_args.extend(injection.args);
    }
    // Codex needs more than an endpoint before a routed model will answer: a
    // provider block, an API-key auth preference, and a catalog entry it is
    // willing to describe. Read from `merged_env`, where both inputs now live —
    // the preset's opt-in knobs and the endpoint the routing above just wrote.
    extra_args.extend(
        crate::codex_overrides::launch_args(spec.provider, spec.model.as_deref(), &merged_env)
            .map_err(|error| error.to_string())?,
    );
    // Point the harness at Medulla's own skills root, when one has been
    // installed. This is what makes a workflow a harness *can* trigger visible
    // to it: the MCP tools arrive automatically, but nothing tells a session
    // that `babysit` exists or what it takes. Empty when no managed skill has
    // been installed for this provider, so the argv is unchanged for anyone who
    // has not run `medulla skills install --scope managed`.
    #[cfg(feature = "workflows")]
    extra_args.extend(crate::workflows::skills::spawn_args(
        spec.provider,
        &spec.env,
    ));
    extra_args.extend(spec.extra_args.iter().cloned());
    let args = build_resumed_run_args(
        spec.provider,
        &spec.prompt,
        spec.model.as_deref(),
        spec.agent.as_deref(),
        &extra_args,
        spec.skip_permissions,
        spec.resume_session_id.as_deref(),
    );
    let bin = provider_bin(spec.provider, &spec.env);

    let stdin_mode = if supports_stdin(spec.provider) {
        Stdio::piped()
    } else {
        Stdio::null()
    };
    let mut command = Command::new(&bin);
    command
        .args(&args)
        .current_dir(&spec.cwd)
        .env_clear()
        .envs(&merged_env)
        .stdin(stdin_mode)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    // ETXTBSY (26) is a transient unix race: a concurrently forked process can
    // briefly hold a freshly written executable's fd open when we exec it.
    // Retry briefly, like the transient-lock retry above.
    let mut spawn_tries = 0u32;
    let mut child = loop {
        match command.spawn() {
            Ok(child) => break child,
            Err(err)
                if err.raw_os_error() == Some(26)
                    && spawn_tries < 50
                    && !spec.abort.is_aborted() =>
            {
                spawn_tries += 1;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(err) => {
                return Err(format!(
                    "failed to start {bin}: {}",
                    with_auth_hint(&err.to_string())
                ));
            }
        }
    };

    // stdin forwarding: hand the caller an unbounded sender; a background task
    // drains it into the child's stdin (appending a newline when missing).
    if let (Some(register), Some(stdin)) = (on_stdin, child.stdin.take()) {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        register(tx);
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(text) = rx.recv().await {
                let line = if text.ends_with('\n') {
                    text
                } else {
                    format!("{text}\n")
                };
                if stdin.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                let _ = stdin.flush().await;
            }
        });
    }

    let stdout = child.stdout.take().ok_or("child has no stdout")?;
    let stderr = child.stderr.take().ok_or("child has no stderr")?;

    // stderr tail collector.
    let stderr_tail = Arc::new(Mutex::new(String::new()));
    let stderr_task = {
        let stderr_tail = stderr_tail.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf).await {
                    Ok(0) => break,
                    Ok(_) => {
                        let chunk = String::from_utf8_lossy(&buf);
                        let mut tail = stderr_tail.lock().unwrap();
                        tail.push_str(&chunk);
                        *tail = tail_bytes(&tail);
                    }
                    Err(_) => break,
                }
            }
        })
    };

    let mut reader = BufReader::new(stdout);
    // PR attribution must use the environment actually installed on the child,
    // not the daemon host's environment. Embedders may add or remove GH_REPO in
    // `spec.env`, and either case changes whether a reported PR is authoritative
    // for the current checkout.
    let mut mapper = HarnessLineMapper::new_with_gh_repo_override(
        provider_name(spec.provider),
        merged_env.contains_key("GH_REPO"),
    );
    mapper.set_workspace_context(
        spec.workspace_context.cwd.clone(),
        spec.workspace_context.branch.clone(),
        spec.workspace_context.pull_request.clone(),
    );
    let mut messages: Vec<String> = Vec::new();
    let mut claude_result: Option<String> = None;
    // First announcement wins — a run reports exactly one session.
    let mut session_id: Option<String> = None;
    let mut events: usize = 0;
    let mut line_no: i64 = 0;
    let mut stdout_tail = String::new();

    // Idle watchdog: killed only after `timeout_ms` with NO new event; each event
    // pushes the deadline out. Armed at start to cover a child that emits nothing.
    let mut deadline = Instant::now() + Duration::from_millis(spec.timeout_ms);
    let mut buf = Vec::new();

    let idle_error = format!(
        "{} task idle for {}ms (no events)",
        provider_name(spec.provider),
        spec.timeout_ms
    );

    loop {
        buf.clear();
        tokio::select! {
            _ = spec.abort.cancelled() => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                report_workspace_context(&mapper, spec);
                return Err(format!("{} task aborted", provider_name(spec.provider)));
            }
            _ = tokio::time::sleep_until(deadline) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                report_workspace_context(&mapper, spec);
                return Err(idle_error);
            }
            read = reader.read_until(b'\n', &mut buf) => {
                match read {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        if buf.len() > MAX_RECORD_BYTES {
                            continue; // unparseable oversized record — drop it.
                        }
                        let raw = String::from_utf8_lossy(&buf);
                        let raw = raw.trim_end_matches(['\n', '\r']);
                        stdout_tail.push_str(raw);
                        stdout_tail.push('\n');
                        stdout_tail = tail_bytes(&stdout_tail);
                        if session_id.is_none() {
                            if let Some(discovered) = extract_session_id(spec.provider, raw) {
                                // Bind before folding this same record: it may
                                // also carry the first workspace update, whose
                                // persistence requires an existing session.
                                if let Some(callback) = on_session.take() {
                                    callback(discovered.clone());
                                }
                                session_id = Some(discovered);
                            }
                        }
                        let produced = consume_line(
                            spec.provider,
                            raw,
                            line_no,
                            &mut mapper,
                            &mut messages,
                            &mut claude_result,
                            &mut events,
                            on_event.as_mut(),
                        );
                        line_no += 1;
                        if produced {
                            deadline = Instant::now() + Duration::from_millis(spec.timeout_ms);
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    }

    let status = child.wait().await;
    // Join the stderr reader before snapshotting the tail: on a fast-exiting
    // child the pipe may not be drained yet, and a lost stderr tail hides the
    // transient-lock marker the retry loop keys on.
    let _ = tokio::time::timeout(Duration::from_millis(500), stderr_task).await;
    if spec.abort.is_aborted() {
        report_workspace_context(&mapper, spec);
        return Err(format!("{} task aborted", provider_name(spec.provider)));
    }
    let code = status.ok().and_then(|s| s.code());
    if let Some(code) = code {
        if code != 0 {
            let stderr = stderr_tail.lock().unwrap().clone();
            let tail = if stderr.trim().is_empty() {
                stdout_tail.clone()
            } else {
                stderr
            };
            let tail: String = tail
                .chars()
                .rev()
                .take(600)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            report_workspace_context(&mapper, spec);
            return Err(format!(
                "{} exited {code}: {}",
                provider_name(spec.provider),
                with_auth_hint(tail.trim())
            ));
        }
    }

    let reply = non_empty(claude_result.as_deref().map(str::trim))
        .or_else(|| non_empty(Some(messages.join("\n").trim())))
        .or_else(|| non_empty(Some(stdout_tail.trim())))
        .unwrap_or_else(|| {
            format!(
                "{} completed without a text response.",
                provider_name(spec.provider)
            )
        });

    report_workspace_context(&mapper, spec);
    Ok(RunTaskResult {
        provider: spec.provider,
        reply,
        events,
        usage: mapper.usage(),
        session_id,
    })
}

/// Persist mapper state on every post-spawn terminal path, including errors.
fn report_workspace_context(mapper: &HarnessLineMapper, spec: &RunSpec) {
    let (cwd, branch, pull_request) = mapper.workspace_context();
    if let Some(callback) = spec.on_workspace_context.as_ref() {
        callback(crate::sessions::WorkspaceContext {
            cwd,
            branch,
            pull_request,
        });
    }
}

/// Fold one raw JSONL line through the mapper, updating the accumulated reply
/// sources and firing the status callback; returns whether it produced events.
#[allow(clippy::too_many_arguments)]
fn consume_line(
    provider: HarnessProvider,
    raw: &str,
    line_no: i64,
    mapper: &mut HarnessLineMapper,
    messages: &mut Vec<String>,
    claude_result: &mut Option<String>,
    events: &mut usize,
    mut on_event: Option<&mut OnEvent>,
) -> bool {
    if raw.trim().is_empty() {
        return false;
    }
    if provider == HarnessProvider::Claude {
        if let Some(result) = extract_claude_result(raw) {
            *claude_result = Some(result);
        }
    }
    let mapped = mapper.map_line(raw, line_no);
    let produced = !mapped.is_empty();
    for semantic in mapped {
        *events += 1;
        if let Some(cb) = on_event.as_deref_mut() {
            cb(&semantic);
        }
        if semantic.event.kind == "agent_message" {
            if let Some(text) = semantic.event.payload.get("text").and_then(|v| v.as_str()) {
                messages.push(text.to_string());
            }
        }
    }
    produced
}

/// Parse a claude stream-json `result` line into its answer text.
pub(super) fn extract_claude_result(raw: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(raw).ok()?;
    if parsed.get("type").and_then(|v| v.as_str()) == Some("result") {
        if let Some(result) = parsed.get("result").and_then(|v| v.as_str()) {
            return Some(result.to_string());
        }
    }
    None
}

/// Return `value` as an owned string only when it is non-empty.
pub(super) fn non_empty(value: Option<&str>) -> Option<String> {
    value.filter(|s| !s.is_empty()).map(str::to_string)
}

/// Retain only the trailing [`TAIL_CAP`] bytes of `value`, clamped to a char
/// boundary.
pub(super) fn tail_bytes(value: &str) -> String {
    if value.len() <= TAIL_CAP {
        return value.to_string();
    }
    let mut start = value.len() - TAIL_CAP;
    while start < value.len() && !value.is_char_boundary(start) {
        start += 1;
    }
    value[start..].to_string()
}
