//! Provider detection + headless one-shot task execution, ported from the
//! tinyplace CLI `daemon/providers.ts`.
//!
//! The daemon runs a delegated task by spawning the requested coding-agent CLI
//! once, non-interactively, and folding its streaming JSONL output through the
//! shared [`super::mappers`] semantic-event mappers to derive status updates and
//! the final agent message. This is the headless complement to the interactive
//! PTY wrapper (which lands separately).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, Notify};
use tokio::time::Instant;

use crate::tinyplace_support::HarnessProvider;

use super::mappers::{HarnessLineMapper, HarnessSemanticEvent};

/// A per-event status callback (drives daemon status frames).
pub type OnEvent = Box<dyn FnMut(&HarnessSemanticEvent) + Send>;
/// A one-shot registration of a child-stdin sender for `input` forwarding.
pub type OnStdin = Box<dyn FnOnce(mpsc::UnboundedSender<String>) + Send>;

/// Every daemon-supported provider.
pub const DAEMON_PROVIDERS: [HarnessProvider; 3] = [
    HarnessProvider::Claude,
    HarnessProvider::Codex,
    HarnessProvider::Opencode,
];

/// A record that never terminates in a newline is dropped past this size.
const MAX_RECORD_BYTES: usize = 1_048_576;
const TAIL_CAP: usize = 8192;
const LOCK_RETRY_ATTEMPTS: u32 = 5;
const LOCK_RETRY_BASE_MS: u64 = 250;

/// A cooperative abort handle shared between the daemon and a running task.
/// Aborting sets the flag and wakes any waiter; a task selects on
/// [`Abort::cancelled`] to terminate its child (SIGTERM).
#[derive(Clone, Default)]
pub struct Abort {
    flag: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl Abort {
    pub fn new() -> Self {
        Self::default()
    }

    /// Signal cancellation.
    pub fn abort(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// Whether cancellation has been signalled.
    pub fn is_aborted(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Resolve once cancellation is signalled (immediately if already aborted).
    pub async fn cancelled(&self) {
        loop {
            if self.is_aborted() {
                return;
            }
            let notified = self.notify.notified();
            if self.is_aborted() {
                return;
            }
            notified.await;
        }
    }
}

/// Resolve the binary name/path for a provider (env override wins). Delegates to
/// the central resolver ([`crate::tinyplace_support::env::provider_bin`]) so the
/// daemon and wrapper share one bin-override contract.
pub fn provider_bin(provider: HarnessProvider, env: &HashMap<String, String>) -> String {
    crate::tinyplace_support::env::provider_bin(provider, env)
}

/// A PATH-lookup predicate (injectable for tests).
pub type ExistsOnPath = Box<dyn Fn(&str) -> bool + Send + Sync>;

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
            return is_executable(std::path::Path::new(bin));
        }
        dirs.iter()
            .any(|dir| is_executable(&std::path::Path::new(dir).join(bin)))
    })
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

/// Build the argv for a one-shot headless run of `provider`.
pub fn build_run_args(
    provider: HarnessProvider,
    prompt: &str,
    model: Option<&str>,
    agent: Option<&str>,
    extra_args: &[String],
    skip_permissions: bool,
) -> Vec<String> {
    // A prompt beginning with "-" would be parsed as a flag by the provider (an
    // injection vector since task text is remote-controlled); neutralize with a
    // leading space, which the model sees as insignificant.
    let prompt = if prompt.starts_with('-') {
        format!(" {prompt}")
    } else {
        prompt.to_string()
    };
    let mut args: Vec<String> = Vec::new();
    match provider {
        HarnessProvider::Claude => {
            args.extend(["-p", "--output-format", "stream-json", "--verbose"].map(String::from));
            if skip_permissions {
                args.push("--dangerously-skip-permissions".to_string());
            }
            args.extend(extra_args.iter().cloned());
            args.push(prompt);
        }
        HarnessProvider::Codex => {
            args.push("exec".to_string());
            args.push("--json".to_string());
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
    }
    args
}

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

/// Inputs for one headless run.
pub struct RunTaskOptions {
    pub provider: HarnessProvider,
    pub prompt: String,
    pub cwd: String,
    pub env: HashMap<String, String>,
    pub timeout_ms: u64,
    pub model: Option<String>,
    pub agent: Option<String>,
    pub extra_args: Vec<String>,
    pub skip_permissions: bool,
    pub abort: Abort,
    /// Fired for each parsed semantic event — drives periodic status frames.
    pub on_event: Option<OnEvent>,
    /// Register a stdin channel for `input`-frame forwarding into the child.
    pub on_stdin: Option<OnStdin>,
}

/// The outcome of a headless run.
#[derive(Debug, Clone)]
pub struct RunTaskResult {
    pub provider: HarnessProvider,
    /// The agent's final answer (concatenated assistant text, or a fallback).
    pub reply: String,
    /// Count of semantic events observed.
    pub events: usize,
}

/// The injectable executor signature (the daemon runtime defaults to
/// [`run_provider_task`]; tests supply a fake).
pub type RunTaskFn = Arc<
    dyn Fn(RunTaskOptions) -> Pin<Box<dyn Future<Output = Result<RunTaskResult, String>> + Send>>
        + Send
        + Sync,
>;

/// The plain-data slice of a run (no callbacks), so it stays `Send + Sync` and a
/// borrow of it can live across the child-process awaits.
struct RunSpec {
    provider: HarnessProvider,
    prompt: String,
    cwd: String,
    env: HashMap<String, String>,
    timeout_ms: u64,
    model: Option<String>,
    agent: Option<String>,
    extra_args: Vec<String>,
    skip_permissions: bool,
    abort: Abort,
}

/// Run one delegated task headlessly, retrying transient opencode SQLite-lock
/// exits with jittered exponential backoff.
pub async fn run_provider_task(options: RunTaskOptions) -> Result<RunTaskResult, String> {
    let mut on_event = options.on_event;
    let mut on_stdin = options.on_stdin;
    let spec = RunSpec {
        provider: options.provider,
        prompt: options.prompt,
        cwd: options.cwd,
        env: options.env,
        timeout_ms: options.timeout_ms,
        model: options.model,
        agent: options.agent,
        extra_args: options.extra_args,
        skip_permissions: options.skip_permissions,
        abort: options.abort,
    };
    let mut attempt: u32 = 1;
    loop {
        // Callbacks are single-use; the retry path (opencode lock) runs without
        // them, mirroring the rarity of that branch.
        let attempt_on_event = on_event.take();
        let attempt_on_stdin = on_stdin.take();
        match run_provider_attempt(&spec, attempt_on_event, attempt_on_stdin).await {
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
fn rand_unit() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos % 1_000_000) as f64 / 1_000_000.0
}

async fn run_provider_attempt(
    spec: &RunSpec,
    mut on_event: Option<OnEvent>,
    on_stdin: Option<OnStdin>,
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
    let mut extra_args = crate::tinyplace_support::env::provider_args(spec.provider, &spec.env);
    extra_args.extend(spec.extra_args.iter().cloned());
    let args = build_run_args(
        spec.provider,
        &spec.prompt,
        spec.model.as_deref(),
        spec.agent.as_deref(),
        &extra_args,
        spec.skip_permissions,
    );
    let bin = provider_bin(spec.provider, &spec.env);

    let mut command = Command::new(&bin);
    command
        .args(&args)
        .current_dir(&spec.cwd)
        .env_clear()
        .envs(&spec.env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            return Err(format!(
                "failed to start {bin}: {}",
                with_auth_hint(&err.to_string())
            ));
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
    let mut mapper = HarnessLineMapper::new(provider_name(spec.provider));
    let mut messages: Vec<String> = Vec::new();
    let mut claude_result: Option<String> = None;
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
                return Err(format!("{} task aborted", provider_name(spec.provider)));
            }
            _ = tokio::time::sleep_until(deadline) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
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

    Ok(RunTaskResult {
        provider: spec.provider,
        reply,
        events,
    })
}

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
fn extract_claude_result(raw: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(raw).ok()?;
    if parsed.get("type").and_then(|v| v.as_str()) == Some("result") {
        if let Some(result) = parsed.get("result").and_then(|v| v.as_str()) {
            return Some(result.to_string());
        }
    }
    None
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value.filter(|s| !s.is_empty()).map(str::to_string)
}

fn tail_bytes(value: &str) -> String {
    if value.len() <= TAIL_CAP {
        return value.to_string();
    }
    let mut start = value.len() - TAIL_CAP;
    while start < value.len() && !value.is_char_boundary(start) {
        start += 1;
    }
    value[start..].to_string()
}

/// The wire name for a provider.
pub fn provider_name(provider: HarnessProvider) -> &'static str {
    provider.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_run_args_per_provider() {
        assert_eq!(
            build_run_args(HarnessProvider::Claude, "hello", None, None, &[], false),
            vec!["-p", "--output-format", "stream-json", "--verbose", "hello"]
        );
        assert_eq!(
            build_run_args(HarnessProvider::Claude, "hi", None, None, &[], true),
            vec![
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--dangerously-skip-permissions",
                "hi"
            ]
        );
        assert_eq!(
            build_run_args(
                HarnessProvider::Codex,
                "do",
                Some("gpt-5"),
                None,
                &[],
                false
            ),
            vec!["exec", "--json", "-m", "gpt-5", "do"]
        );
        assert_eq!(
            build_run_args(
                HarnessProvider::Opencode,
                "do",
                None,
                Some("plan"),
                &[],
                false
            ),
            vec!["run", "--agent", "plan", "--format", "json", "do"]
        );
    }

    #[test]
    fn build_run_args_neutralizes_dash_prompt() {
        let args = build_run_args(HarnessProvider::Codex, "-rf /", None, None, &[], false);
        assert_eq!(args.last().unwrap(), " -rf /");
    }

    #[test]
    fn provider_bin_env_override_wins() {
        let mut env = HashMap::new();
        env.insert("TINYPLACE_CODEX_BIN".to_string(), "/opt/codex".to_string());
        assert_eq!(provider_bin(HarnessProvider::Codex, &env), "/opt/codex");
        assert_eq!(provider_bin(HarnessProvider::Claude, &env), "claude");
    }

    #[test]
    fn detect_providers_uses_injected_lookup() {
        let env = HashMap::new();
        let lookup: ExistsOnPath = Box::new(|bin: &str| bin == "codex");
        let detected = detect_providers(&env, None, Some(&lookup));
        assert_eq!(detected, vec![HarnessProvider::Codex]);
    }

    #[test]
    fn transient_lock_and_auth_hint() {
        assert!(is_transient_lock("SQLITE_BUSY: database is locked"));
        assert!(is_transient_lock("Error: database is locked"));
        assert!(is_transient_lock("database table is locked"));
        assert!(!is_transient_lock("some other error"));
        assert!(with_auth_hint("unexpected server error").contains("opencode auth login"));
        assert!(with_auth_hint("HTTP 401 Unauthorized").contains("opencode auth login"));
        assert!(with_auth_hint("missing api key").contains("opencode auth login"));
        assert!(with_auth_hint("bad credential").contains("opencode auth login"));
        assert_eq!(with_auth_hint("plain failure"), "plain failure");
    }

    #[test]
    fn build_run_args_opencode_with_model_and_extra() {
        let args = build_run_args(
            HarnessProvider::Opencode,
            "task",
            Some("anthropic/claude"),
            Some("build"),
            &["--foo".to_string()],
            false,
        );
        assert_eq!(
            args,
            vec![
                "run",
                "-m",
                "anthropic/claude",
                "--agent",
                "build",
                "--format",
                "json",
                "--foo",
                "task",
            ]
        );
    }

    #[test]
    fn build_run_args_claude_extra_and_dash_prompt() {
        let args = build_run_args(
            HarnessProvider::Claude,
            "-hi",
            None,
            None,
            &["--mcp".to_string()],
            true,
        );
        // extra args precede the (space-neutralized) prompt.
        assert_eq!(args[args.len() - 2], "--mcp");
        assert_eq!(args.last().unwrap(), " -hi");
        assert!(args.contains(&"--dangerously-skip-permissions".to_string()));
    }

    #[test]
    fn provider_bin_prefers_first_env_key_and_trims() {
        let mut env = HashMap::new();
        // Claude honors TINYVERSE_* before TINYPLACE_*.
        env.insert(
            "TINYVERSE_CLAUDE_BIN".to_string(),
            "  /opt/claude  ".to_string(),
        );
        env.insert(
            "TINYPLACE_CLAUDE_BIN".to_string(),
            "/other/claude".to_string(),
        );
        assert_eq!(provider_bin(HarnessProvider::Claude, &env), "/opt/claude");

        // A whitespace-only override is ignored (falls back to the default).
        let mut blank = HashMap::new();
        blank.insert("TINYPLACE_CODEX_BIN".to_string(), "   ".to_string());
        assert_eq!(provider_bin(HarnessProvider::Codex, &blank), "codex");
    }

    #[test]
    fn provider_names_are_wire_stable() {
        assert_eq!(provider_name(HarnessProvider::Claude), "claude");
        assert_eq!(provider_name(HarnessProvider::Codex), "codex");
        assert_eq!(provider_name(HarnessProvider::Opencode), "opencode");
    }

    #[test]
    fn non_empty_and_tail_bytes_helpers() {
        assert_eq!(non_empty(Some("hi")).as_deref(), Some("hi"));
        assert_eq!(non_empty(Some("")), None);
        assert_eq!(non_empty(None), None);

        let small = "abc";
        assert_eq!(tail_bytes(small), "abc");
        let big = "x".repeat(TAIL_CAP + 100);
        let tail = tail_bytes(&big);
        assert_eq!(tail.len(), TAIL_CAP);
    }

    #[test]
    fn rand_unit_is_in_range() {
        let value = rand_unit();
        assert!((0.0..1.0).contains(&value));
    }

    #[test]
    fn extract_claude_result_reads_result_line() {
        let line = r#"{"type":"result","result":"the answer"}"#;
        assert_eq!(extract_claude_result(line).as_deref(), Some("the answer"));
        // A non-result line yields nothing.
        assert_eq!(extract_claude_result(r#"{"type":"assistant"}"#), None);
        assert_eq!(extract_claude_result("not json"), None);
    }

    #[test]
    fn make_path_lookup_resolves_pathish_and_bare_names() {
        // A path-ish name is probed directly; a missing one is not executable.
        let env = HashMap::new();
        let lookup = make_path_lookup(&env);
        assert!(!lookup("/nonexistent/definitely-not-here"));

        // A bare name is searched across PATH; an empty PATH finds nothing.
        assert!(!lookup("definitely-not-a-real-binary-xyz"));
    }

    #[tokio::test]
    async fn abort_cancelled_resolves_when_signalled() {
        let abort = Abort::new();
        assert!(!abort.is_aborted());
        let waiter = abort.clone();
        let handle = tokio::spawn(async move { waiter.cancelled().await });
        abort.abort();
        assert!(abort.is_aborted());
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("cancelled should resolve")
            .unwrap();
        // Already-aborted: cancelled returns immediately.
        abort.cancelled().await;
    }
}
