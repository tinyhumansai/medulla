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
        on_event: None,
        on_stdin: None,
    };
    let err = run_provider_task(options).await.expect_err("aborted");
    assert!(err.contains("aborted"), "got: {err}");
    assert!(!err.contains("idle"), "abort beat the idle watchdog: {err}");
}

#[tokio::test]
async fn stdin_input_reaches_child_and_echoes_in_reply() {
    // Opencode is excluded: `opencode run` treats a non-TTY stdin as prompt
    // content and blocks until EOF, so the daemon gives it a null stdin and has
    // no mid-run stdin channel for it (see opencode_stdin_is_immediate_eof).
    for provider in [MockProvider::Claude, MockProvider::Codex] {
        let dir = MockDir::new();
        let mock = MockCli::new(provider).stdin_echo();
        let env = dir.env_for(&mock);
        let stdin_tx: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedSender<String>>>> =
            Arc::new(Mutex::new(None));
        let register = stdin_tx.clone();
        let options = RunTaskOptions {
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
async fn opencode_stdin_is_immediate_eof() {
    // The daemon spawns opencode with a null stdin (piping would deadlock the
    // real CLI, which reads a non-TTY stdin as prompt content until EOF). The
    // stdin-echo mock's `read` must therefore see instant EOF — an empty line —
    // and no stdin sender is ever registered for `input` forwarding.
    let provider = MockProvider::Opencode;
    let dir = MockDir::new();
    let mock = MockCli::new(provider).stdin_echo();
    let env = dir.env_for(&mock);
    let registered = Arc::new(Mutex::new(false));
    let register = registered.clone();
    let options = RunTaskOptions {
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
        on_event: None,
        on_stdin: Some(Box::new(move |_tx| {
            *register.lock().unwrap() = true;
        })),
    };
    let run = run_provider_task(options)
        .await
        .unwrap_or_else(|e| panic!("opencode failed: {e}"));
    assert_eq!(run.reply, "got:", "stdin was not immediate-EOF");
    assert!(
        !*registered.lock().unwrap(),
        "no stdin sender should be registered for opencode"
    );
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
