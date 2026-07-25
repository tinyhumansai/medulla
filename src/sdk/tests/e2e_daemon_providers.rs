//! (Unix-only: exercises Unix-domain-socket cores and/or spawned `/bin/sh` mock scripts.)
#![cfg(unix)]

//! End-to-end coverage for the daemon's provider spawn path
//! ([`medulla::daemon::providers::run_provider_task`]) driven by the mock
//! coding-agent CLIs in [`mock_harness`]. Each test installs a scripted mock as
//! the provider binary (via `TINYPLACE_*_BIN`) and asserts the derived semantic
//! events, reply extraction, and error branches — with no real CLI and no
//! network.

mod support;

#[path = "support/mock_harness.rs"]
mod mock_harness;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use medulla::daemon::providers::{
    provider_bin, run_provider_task, Abort, RunTaskOptions, RunTaskResult,
};
use medulla::tinyplace::HarnessProvider;

use mock_harness::{
    auth_failure, garbage_then_reply, hang, success, tool_workflow, MockCli, MockDir, MockProvider,
};

fn harness(p: MockProvider) -> HarnessProvider {
    match p {
        MockProvider::Claude => HarnessProvider::Claude,
        MockProvider::Codex => HarnessProvider::Codex,
        MockProvider::Opencode => HarnessProvider::Opencode,
    }
}

/// Run one mock CLI through the real spawn path, collecting semantic-event kinds.
async fn run(
    mock: &MockCli,
    provider: MockProvider,
    dir: &MockDir,
    prompt: &str,
    timeout_ms: u64,
) -> (Result<RunTaskResult, String>, Vec<String>) {
    let env = dir.env_for(mock);
    let kinds = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = kinds.clone();
    let options = RunTaskOptions {
        conversation: String::new(),
        resume_session_id: None,
        provider: harness(provider),
        prompt: prompt.to_string(),
        cwd: ".".to_string(),
        env,
        timeout_ms,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
        abort: Abort::new(),
        router: None,
        on_event: Some(Box::new(move |ev| {
            sink.lock().unwrap().push(ev.event.kind.clone());
        })),
        on_stdin: None,
    };
    let result = run_provider_task(options).await;
    let events = kinds.lock().unwrap().clone();
    (result, events)
}

const PROVIDERS: [MockProvider; 3] = [
    MockProvider::Claude,
    MockProvider::Codex,
    MockProvider::Opencode,
];

#[tokio::test]
async fn success_reply_per_provider() {
    for provider in PROVIDERS {
        let dir = MockDir::new();
        let mock = success(provider, "final answer");
        let (result, events) = run(&mock, provider, &dir, "do it", 5_000).await;
        let run = result.unwrap_or_else(|e| panic!("{provider:?} failed: {e}"));
        assert_eq!(run.reply, "final answer", "{provider:?}");
        assert!(
            events.iter().any(|k| k == "agent_message"),
            "{provider:?} emitted an agent_message: {events:?}"
        );
    }
}

#[tokio::test]
async fn tool_workflow_events_per_provider() {
    for provider in PROVIDERS {
        let dir = MockDir::new();
        let mock = tool_workflow(provider, "done");
        let (result, events) = run(&mock, provider, &dir, "work", 5_000).await;
        let run = result.unwrap_or_else(|e| panic!("{provider:?} failed: {e}"));
        assert_eq!(run.reply, "done", "{provider:?}");
        assert!(events.iter().any(|k| k == "tool_call"), "{provider:?}");
        assert!(events.iter().any(|k| k == "tool_result"), "{provider:?}");
        assert!(
            events.iter().any(|k| k == "agent_thinking"),
            "{provider:?} thinking: {events:?}"
        );
    }
}

#[tokio::test]
async fn garbage_lines_are_dropped_reply_still_extracted() {
    for provider in PROVIDERS {
        let dir = MockDir::new();
        let mock = garbage_then_reply(provider, "survived the noise");
        let (result, _events) = run(&mock, provider, &dir, "work", 5_000).await;
        let run = result.unwrap_or_else(|e| panic!("{provider:?} failed: {e}"));
        assert_eq!(run.reply, "survived the noise", "{provider:?}");
    }
}

#[tokio::test]
async fn idle_watchdog_kills_hung_provider() {
    for provider in PROVIDERS {
        let dir = MockDir::new();
        let mock = hang(provider);
        let (result, _events) = run(&mock, provider, &dir, "wait", 250).await;
        let err = result.expect_err("hang should time out");
        assert!(err.contains("idle"), "{provider:?} idle error: {err}");
    }
}

#[tokio::test]
async fn nonzero_exit_surfaces_stderr_with_auth_hint() {
    for provider in PROVIDERS {
        let dir = MockDir::new();
        let mock = auth_failure(provider);
        let (result, _events) = run(&mock, provider, &dir, "work", 5_000).await;
        let err = result.expect_err("non-zero exit should error");
        assert!(
            err.contains("exited 1"),
            "{provider:?} exit surfaced: {err}"
        );
        assert!(
            err.contains("opencode auth login"),
            "{provider:?} auth hint appended: {err}"
        );
    }
}

#[tokio::test]
async fn spawn_failure_for_missing_binary() {
    // A binary that does not exist → spawn error, annotated by provider_bin.
    let mut env = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert(
        "TINYPLACE_CLAUDE_BIN".to_string(),
        "/nonexistent/definitely-not-here".to_string(),
    );
    assert_eq!(
        provider_bin(HarnessProvider::Claude, &env),
        "/nonexistent/definitely-not-here"
    );
    let options = RunTaskOptions {
        conversation: String::new(),
        resume_session_id: None,
        provider: HarnessProvider::Claude,
        prompt: "x".to_string(),
        cwd: ".".to_string(),
        env,
        timeout_ms: 1_000,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
        abort: Abort::new(),
        router: None,
        on_event: None,
        on_stdin: None,
    };
    let err = run_provider_task(options)
        .await
        .expect_err("spawn should fail");
    assert!(err.contains("failed to start"), "got: {err}");
}

#[tokio::test]
async fn abort_before_start_returns_immediately() {
    let dir = MockDir::new();
    let mock = success(MockProvider::Claude, "never runs");
    let env = dir.env_for(&mock);
    let abort = Abort::new();
    abort.abort();
    let options = RunTaskOptions {
        conversation: String::new(),
        resume_session_id: None,
        provider: HarnessProvider::Claude,
        prompt: "x".to_string(),
        cwd: ".".to_string(),
        env,
        timeout_ms: 5_000,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
        abort,
        router: None,
        on_event: None,
        on_stdin: None,
    };
    let err = run_provider_task(options).await.expect_err("aborted");
    assert!(err.contains("aborted before start"), "got: {err}");
}

#[tokio::test]
async fn abort_mid_run_kills_child() {
    // A hanging mock; abort after it starts so the cancellation branch (not the
    // idle deadline) terminates the child.
    let dir = MockDir::new();
    let mock = hang(MockProvider::Claude);
    let env = dir.env_for(&mock);
    let abort = Abort::new();
    let abort_bg = abort.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        abort_bg.abort();
    });
    let options = RunTaskOptions {
        conversation: String::new(),
        resume_session_id: None,
        provider: HarnessProvider::Claude,
        prompt: "x".to_string(),
        cwd: ".".to_string(),
        env,
        timeout_ms: 30_000, // long, so the abort (not idle) ends the run
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
        abort,
        router: None,
        on_event: None,
        on_stdin: None,
    };
    let err = run_provider_task(options).await.expect_err("aborted");
    assert!(err.contains("aborted"), "got: {err}");
    assert!(!err.contains("idle"), "abort beat the idle watchdog: {err}");
}

#[tokio::test]
async fn stdin_input_reaches_child_and_echoes_in_reply() {
    // Only claude forwards mid-run stdin. Opencode and codex both read a non-TTY
    // stdin as prompt content and block until EOF, so the daemon gives them a null
    // stdin and has no mid-run channel (see stdin_is_immediate_eof_for_batch_cli).
    for provider in [MockProvider::Claude] {
        let dir = MockDir::new();
        let mock = MockCli::new(provider).stdin_echo();
        let env = dir.env_for(&mock);
        let stdin_tx: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedSender<String>>>> =
            Arc::new(Mutex::new(None));
        let register = stdin_tx.clone();
        let options = RunTaskOptions {
            conversation: String::new(),
            resume_session_id: None,
            provider: harness(provider),
            prompt: "start".to_string(),
            cwd: ".".to_string(),
            env,
            timeout_ms: 5_000,
            model: None,
            agent: None,
            extra_args: Vec::new(),
            skip_permissions: false,
            abort: Abort::new(),
            router: None,
            on_event: None,
            on_stdin: Some(Box::new(move |tx| {
                *register.lock().unwrap() = Some(tx);
            })),
        };
        // Feed stdin shortly after the run starts.
        let feeder = stdin_tx.clone();
        tokio::spawn(async move {
            for _ in 0..50 {
                if let Some(tx) = feeder.lock().unwrap().as_ref() {
                    let _ = tx.send("guidance".to_string());
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        let run = run_provider_task(options)
            .await
            .unwrap_or_else(|e| panic!("{provider:?} failed: {e}"));
        assert_eq!(run.reply, "got: guidance", "{provider:?}");
    }
}

#[tokio::test]
async fn stdin_is_immediate_eof_for_batch_cli() {
    // The daemon spawns opencode AND codex with a null stdin: both `opencode run`
    // and `codex exec` read a non-TTY stdin as prompt content and block until EOF,
    // so piping would deadlock the real CLI. The stdin-echo mock's `read` must
    // therefore see instant EOF — an empty line — and no stdin sender is ever
    // registered for `input` forwarding.
    for provider in [MockProvider::Opencode, MockProvider::Codex] {
        let dir = MockDir::new();
        let mock = MockCli::new(provider).stdin_echo();
        let env = dir.env_for(&mock);
        let registered = Arc::new(Mutex::new(false));
        let register = registered.clone();
        let options = RunTaskOptions {
            conversation: String::new(),
            resume_session_id: None,
            provider: harness(provider),
            prompt: "start".to_string(),
            cwd: ".".to_string(),
            env,
            timeout_ms: 5_000,
            model: None,
            agent: None,
            extra_args: Vec::new(),
            skip_permissions: false,
            abort: Abort::new(),
            router: None,
            on_event: None,
            on_stdin: Some(Box::new(move |_tx| {
                *register.lock().unwrap() = true;
            })),
        };
        let run = run_provider_task(options)
            .await
            .unwrap_or_else(|e| panic!("{provider:?} failed: {e}"));
        assert_eq!(
            run.reply, "got:",
            "{provider:?} stdin was not immediate-EOF"
        );
        assert!(
            !*registered.lock().unwrap(),
            "no stdin sender should be registered for {provider:?}"
        );
    }
}

#[tokio::test]
async fn transient_lock_exit_is_retried_to_success() {
    // The opencode SQLite store throws a transient lock on the first spawn; the
    // retry loop backs off and re-runs, and the second spawn succeeds.
    let provider = MockProvider::Opencode;
    let dir = MockDir::new();
    let mock = MockCli::new(provider).flaky_lock("locked then fine");
    let (result, _events) = run(&mock, provider, &dir, "work", 5_000).await;
    let run = result.unwrap_or_else(|e| panic!("retry should succeed: {e}"));
    assert_eq!(run.reply, "locked then fine");
}

#[tokio::test]
async fn codex_dedupes_double_recorded_message_over_real_spawn() {
    // Codex records the same agent message twice (event_msg + response_item); the
    // stateful mapper must dedupe it, so `events` counts one message.
    let provider = MockProvider::Codex;
    let dir = MockDir::new();
    let mock = MockCli::new(provider)
        .message("final answer")
        .step(mock_harness::Step::Raw(
            json!({
                "type": "response_item",
                "timestamp": "2026-07-05T00:00:00.100Z",
                "payload": {
                    "type": "message",
                    "role": "assistant",
                    "content": [{ "type": "output_text", "text": "final answer" }],
                },
            })
            .to_string(),
        ));
    let (result, events) = run(&mock, provider, &dir, "work", 5_000).await;
    let run = result.unwrap();
    assert_eq!(run.reply, "final answer");
    let messages = events.iter().filter(|k| *k == "agent_message").count();
    assert_eq!(messages, 1, "duplicate agent_message deduped: {events:?}");
}

// -------------------------------------------------------------- router ---
//
// Step 3: the custom OpenAI-compatible router injects the provider's endpoint
// env at the spawn seam, resolves the API key from the daemon's own environment
// BY NAME at spawn, and never lets the key value reach the reply frame.

use support::fake_provider::TempDir;

/// Parse a `RouterConfig` from JSON for the spawn-seam tests.
fn router_cfg(json: &str) -> medulla::config::RouterConfig {
    serde_json::from_str(json).expect("valid router config")
}

/// Build an env carrying host `PATH` plus explicit overrides.
fn env_with(overrides: &[(&str, &str)]) -> HashMap<String, String> {
    let mut env = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    for (k, v) in overrides {
        env.insert((*k).to_string(), (*v).to_string());
    }
    env
}

fn router_options(
    provider: HarnessProvider,
    bin: &str,
    env: HashMap<String, String>,
    router: Option<medulla::config::RouterConfig>,
) -> RunTaskOptions {
    let _ = bin;
    RunTaskOptions {
        conversation: String::new(),
        resume_session_id: None,
        provider,
        prompt: "do it".to_string(),
        cwd: ".".to_string(),
        env,
        timeout_ms: 5_000,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
        abort: Abort::new(),
        router,
        on_event: None,
        on_stdin: None,
    }
}

#[tokio::test]
async fn router_injects_claude_endpoint_and_resolves_key_by_name_without_leaking() {
    // A distinctive secret so any leak is unmistakable.
    const SECRET: &str = "sk-router-secret-DO-NOT-LEAK-9f3a";
    let dir = TempDir::new();
    let marker = dir.path().join("key-marker");
    let marker = marker.to_string_lossy().into_owned();
    // The fake claude echoes the ROUTED endpoint into its result (safe to log)
    // and writes the resolved AUTH TOKEN to a marker file — out of band, so the
    // token never rides the reply frame.
    let script = format!(
        "#!/bin/sh\n\
         printf '%s' \"$ANTHROPIC_AUTH_TOKEN\" > '{marker}'\n\
         printf '{{\"type\":\"result\",\"result\":\"endpoint=%s\"}}\\n' \"$ANTHROPIC_BASE_URL\"\n",
    );
    let bin = dir.write_script("router_claude.sh", &script);

    // The daemon's own environment holds the key under its configured name.
    let env = env_with(&[
        ("TINYPLACE_CLAUDE_BIN", &bin),
        ("MEDULLA_ROUTER_KEY", SECRET),
    ]);
    let router =
        router_cfg(r#"{"baseUrl":"https://gw/anthropic","apiKeyEnv":"MEDULLA_ROUTER_KEY"}"#);
    let options = router_options(HarnessProvider::Claude, &bin, env, Some(router));
    let result = run_provider_task(options).await.expect("router run ok");

    // The child saw the routed endpoint...
    assert_eq!(result.reply, "endpoint=https://gw/anthropic");
    // ...and received the key resolved from the daemon env BY NAME.
    let seen = std::fs::read_to_string(&marker).expect("marker written");
    assert_eq!(seen, SECRET, "child received the resolved AUTH token");
    // ...but the secret NEVER appears in the reply frame.
    assert!(
        !result.reply.contains(SECRET),
        "the key value must not leak into the reply frame"
    );
}

#[tokio::test]
async fn router_codex_endpoint_only_preserves_existing_credentials() {
    // No apiKeyEnv → the router steers OPENAI_BASE_URL and leaves the harness's
    // own OPENAI_API_KEY untouched (endpoint-only routing).
    let dir = TempDir::new();
    let marker = dir.path().join("codex-env");
    let marker = marker.to_string_lossy().into_owned();
    let script = format!(
        "#!/bin/sh\n\
         printf 'base=%s key=%s' \"$OPENAI_BASE_URL\" \"$OPENAI_API_KEY\" > '{marker}'\n\
         printf '{{\"type\":\"event_msg\",\"timestamp\":\"2026-07-05T00:00:00Z\",\"payload\":{{\"type\":\"agent_message\",\"message\":\"ok\"}}}}\\n'\n",
    );
    let bin = dir.write_script("router_codex.sh", &script);
    let env = env_with(&[
        ("TINYPLACE_CODEX_BIN", &bin),
        ("OPENAI_API_KEY", "pre-existing-key"),
    ]);
    let router = router_cfg(r#"{"baseUrl":"https://gw/v1"}"#);
    let options = router_options(HarnessProvider::Codex, &bin, env, Some(router));
    let result = run_provider_task(options).await.expect("router run ok");
    assert_eq!(result.reply, "ok");
    let seen = std::fs::read_to_string(&marker).expect("marker written");
    assert_eq!(seen, "base=https://gw/v1 key=pre-existing-key");
}

#[tokio::test]
async fn router_missing_key_env_is_a_hard_error_not_a_silent_empty_key() {
    // apiKeyEnv names a var that is absent from the daemon env → explicit error
    // (surfaced upstream as an error frame), never a silent unauthenticated spawn.
    let env = env_with(&[("TINYPLACE_CODEX_BIN", "/nonexistent/codex")]);
    let router = router_cfg(r#"{"baseUrl":"https://gw/v1","apiKeyEnv":"ABSENT_ROUTER_KEY"}"#);
    let options = router_options(HarnessProvider::Codex, "codex", env, Some(router));
    let err = run_provider_task(options)
        .await
        .expect_err("missing router key must error");
    assert!(
        err.contains("ABSENT_ROUTER_KEY"),
        "names the missing var: {err}"
    );
    assert!(err.contains("not set"), "explains the failure: {err}");
}

#[tokio::test]
async fn no_router_config_spawns_child_unchanged() {
    // router: None → zero behaviour change; the endpoint env is never set.
    let dir = TempDir::new();
    let marker = dir.path().join("no-router");
    let marker = marker.to_string_lossy().into_owned();
    let script = format!(
        "#!/bin/sh\n\
         printf 'base=[%s]' \"$ANTHROPIC_BASE_URL\" > '{marker}'\n\
         printf '{{\"type\":\"result\",\"result\":\"done\"}}\\n'\n",
    );
    let bin = dir.write_script("no_router_claude.sh", &script);
    let env = env_with(&[("TINYPLACE_CLAUDE_BIN", &bin)]);
    let options = router_options(HarnessProvider::Claude, &bin, env, None);
    let result = run_provider_task(options).await.expect("run ok");
    assert_eq!(result.reply, "done");
    let seen = std::fs::read_to_string(&marker).expect("marker written");
    assert_eq!(seen, "base=[]", "no router → ANTHROPIC_BASE_URL is unset");
}

#[tokio::test]
async fn router_loaded_from_config_file_reaches_the_spawned_child() {
    // The config-load → daemon-population wiring, end to end: a `[router]` section
    // on disk is parsed by `load_config` into the exact `RouterConfig` the spawn
    // seam accepts, and its endpoint reaches the child. This is the seam the
    // standalone daemon and worker TUI use to populate `DaemonConfig.router`.
    let dir = TempDir::new();
    // An explicit config file with a router endpoint (camelCase on the wire).
    let config_path = dir.path().join("medulla.tui.json");
    std::fs::write(
        &config_path,
        r#"{"router":{"baseUrl":"https://gw/anthropic","apiKeyEnv":"MEDULLA_ROUTER_KEY"}}"#,
    )
    .expect("write config");

    // Load it the way the daemon does; env only carries a home so discovery is
    // deterministic (the explicit path bypasses layer discovery regardless).
    let load_env: HashMap<String, String> = [(
        "MEDULLA_HOME".to_string(),
        dir.path().to_string_lossy().into_owned(),
    )]
    .into_iter()
    .collect();
    let loaded = medulla::config::load_config(
        Some(config_path.to_string_lossy().as_ref()),
        &load_env,
        dir.path(),
    )
    .expect("config loads");
    let router = loaded
        .config
        .router
        .clone()
        .expect("[router] section populated the daemon config");

    // Feed the loaded router into the real spawn path; the child must see it.
    const SECRET: &str = "sk-from-config-file-2b7c";
    let marker = dir.path().join("cfg-key-marker");
    let marker = marker.to_string_lossy().into_owned();
    let script = format!(
        "#!/bin/sh\n\
         printf '%s' \"$ANTHROPIC_AUTH_TOKEN\" > '{marker}'\n\
         printf '{{\"type\":\"result\",\"result\":\"endpoint=%s\"}}\\n' \"$ANTHROPIC_BASE_URL\"\n",
    );
    let bin = dir.write_script("cfg_router_claude.sh", &script);
    let env = env_with(&[
        ("TINYPLACE_CLAUDE_BIN", &bin),
        ("MEDULLA_ROUTER_KEY", SECRET),
    ]);
    let options = router_options(HarnessProvider::Claude, &bin, env, Some(router));
    let result = run_provider_task(options).await.expect("router run ok");

    assert_eq!(result.reply, "endpoint=https://gw/anthropic");
    let seen = std::fs::read_to_string(&marker).expect("marker written");
    assert_eq!(seen, SECRET, "child received the key resolved by name");
}
