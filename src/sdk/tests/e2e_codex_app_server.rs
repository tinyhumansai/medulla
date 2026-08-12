//! End-to-end tests for the shared `codex app-server` transport, against a
//! scripted fake server.
//!
//! These drive the real execution path — `run_provider_task` with the
//! app-server transport selected — through a real child process, so they cover
//! the spawn, the JSON-RPC handshake, thread setup, the notification fold, and
//! the pool's reuse rule. No network and no real Codex.
//!
//! The claim the transport exists for is *process sharing*, which is only
//! observable across tasks, so most of these assert on how many processes the
//! fake recorded rather than on what one task returned.

mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use medulla::daemon::mappers::HarnessSemanticEvent;
use medulla::daemon::providers::{run_provider_task, Abort, RunTaskOptions, RunTaskResult};
use medulla::protocol::{HarnessProvider, HarnessTransport};
use medulla::sessions::SessionClass;

use support::fake_app_server::{fake_app_server, FakeAppServer, TurnScript};
use support::fake_provider::TempDir;

/// A run against `fake`, with its own Codex home so tests never share a pooled
/// process — the pool is process-global by design, and two tests that keyed the
/// same would see each other's connection.
fn options(
    fake: &FakeAppServer,
    home: &str,
    prompt: &str,
    timeout_ms: u64,
) -> (RunTaskOptions, Arc<Mutex<Vec<HarnessSemanticEvent>>>) {
    let seen: Arc<Mutex<Vec<HarnessSemanticEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let mut env = HashMap::new();
    env.insert("TINYPLACE_CODEX_BIN".to_string(), fake.bin.clone());
    env.insert("CODEX_HOME".to_string(), home.to_string());
    env.insert(
        "PATH".to_string(),
        std::env::var("PATH").unwrap_or_default(),
    );
    (
        RunTaskOptions {
            provider: HarnessProvider::Codex,
            transport: HarnessTransport::AppServer,
            prompt: prompt.to_string(),
            cwd: "/tmp".to_string(),
            env,
            timeout_ms,
            model: None,
            agent: None,
            extra_args: Vec::new(),
            skip_permissions: true,
            conversation: "peer".to_string(),
            session_class: SessionClass::Bounded,
            resume_session_id: None,
            workspace_context: Default::default(),
            abort: Abort::new(),
            router: None,
            attribution: false,
            hooks: Default::default(),
            on_event: Some(Box::new(move |event: &HarnessSemanticEvent| {
                sink.lock().unwrap().push(event.clone());
            })),
            on_stdin: None,
            on_session: None,
            on_workspace_context: None,
        },
        seen,
    )
}

/// A unique Codex home per test, so each gets its own pooled process.
fn home(dir: &TempDir, name: &str) -> String {
    let path = dir.path().join(name);
    std::fs::create_dir_all(&path).unwrap();
    path.to_string_lossy().into_owned()
}

async fn run(fake: &FakeAppServer, home: &str, prompt: &str) -> Result<RunTaskResult, String> {
    let (options, _) = options(fake, home, prompt, 10_000);
    run_provider_task(options).await
}

#[tokio::test]
async fn runs_a_turn_and_reports_the_reply() {
    let dir = TempDir::new();
    let fake = fake_app_server(&dir, TurnScript::Reply("the answer"));
    let (options, seen) = options(&fake, &home(&dir, "a"), "do it", 10_000);

    let result = run_provider_task(options).await.expect("the turn runs");
    assert_eq!(result.reply, "the answer");
    assert_eq!(result.provider, HarnessProvider::Codex);
    assert!(result.session_id.is_some(), "the thread id is captured");

    let usage = result.usage.expect("token usage is folded");
    assert_eq!(usage.input_tokens, 11);
    assert_eq!(usage.output_tokens, 7);

    let events = seen.lock().unwrap();
    let kinds: Vec<&str> = events
        .iter()
        .map(|event| event.event.kind.as_str())
        .collect();
    assert!(kinds.contains(&"agent_message"), "{kinds:?}");
    assert!(kinds.contains(&"status"), "{kinds:?}");
}

#[tokio::test]
async fn resumed_turn_uses_the_worktree_as_its_runtime_cwd() {
    let dir = TempDir::new();
    let worktree = dir.path().join("worktree");
    std::fs::create_dir_all(&worktree).unwrap();
    let fake = fake_app_server(&dir, TurnScript::Reply("ok"));
    let (mut options, _) = options(&fake, &home(&dir, "cwd"), "continue", 10_000);
    options.cwd = dir.path().to_string_lossy().into_owned();
    options.workspace_context.cwd = Some(worktree.to_string_lossy().into_owned());

    run_provider_task(options).await.expect("the turn runs");

    let turn = fake
        .requests()
        .into_iter()
        .find(|request| request["method"] == "turn/start")
        .expect("turn/start request");
    assert_eq!(turn["params"]["cwd"], worktree.to_string_lossy().as_ref());
}

/// The whole point: several tasks, one process.
#[tokio::test]
async fn shares_one_process_across_sequential_tasks() {
    let dir = TempDir::new();
    let fake = fake_app_server(&dir, TurnScript::Reply("ok"));
    let home = home(&dir, "shared");

    for _ in 0..3 {
        run(&fake, &home, "work").await.expect("each turn runs");
    }
    assert_eq!(fake.spawn_count(), 1, "three tasks spawned one process");

    // Three threads, not one reused: two tasks must never see each other's
    // context just because they share a runtime.
    let starts = fake
        .methods()
        .iter()
        .filter(|method| *method == "thread/start")
        .count();
    assert_eq!(starts, 3, "each task got its own thread");
}

#[tokio::test]
async fn shares_one_process_across_concurrent_tasks() {
    let dir = TempDir::new();
    let fake = fake_app_server(&dir, TurnScript::Reply("ok"));
    let home = home(&dir, "concurrent");

    let runs = (0..4).map(|index| {
        let (options, _) = options(&fake, &home, &format!("lane {index}"), 10_000);
        run_provider_task(options)
    });
    for result in futures::future::join_all(runs).await {
        assert_eq!(result.expect("each lane runs").reply, "ok");
    }
    assert_eq!(
        fake.spawn_count(),
        1,
        "four concurrent lanes shared a process"
    );
}

/// Two runs that would authenticate differently must not share a process.
#[tokio::test]
async fn separates_processes_with_different_identities() {
    let dir = TempDir::new();
    let fake = fake_app_server(&dir, TurnScript::Reply("ok"));

    run(&fake, &home(&dir, "one"), "work").await.expect("first");
    run(&fake, &home(&dir, "two"), "work")
        .await
        .expect("second");

    assert_eq!(fake.spawn_count(), 2, "different Codex homes got their own");
}

#[tokio::test]
async fn reports_a_failed_turn_with_its_message() {
    let dir = TempDir::new();
    let fake = fake_app_server(&dir, TurnScript::Fail("context exhausted"));
    let error = run(&fake, &home(&dir, "failing"), "work")
        .await
        .expect_err("the turn fails");
    assert!(error.contains("context exhausted"), "{error}");
}

/// A server that never answers must not pin a lane forever, and the interrupt
/// has to reach the *thread* — killing the process would take every other lane
/// on it with it.
#[tokio::test]
async fn gives_up_on_a_silent_turn_without_killing_the_process() {
    let dir = TempDir::new();
    let fake = fake_app_server(&dir, TurnScript::Hang);
    let error = run(&fake, &home(&dir, "hanging"), "wait")
        .await
        .expect_err("the turn times out");
    assert!(error.contains("idle"), "{error}");

    // Still one live process, and a second task can still use it.
    assert_eq!(fake.spawn_count(), 1);
}

#[tokio::test]
async fn aborts_a_running_turn_on_request() {
    let dir = TempDir::new();
    let fake = fake_app_server(&dir, TurnScript::Hang);
    let (options, _) = options(&fake, &home(&dir, "aborting"), "wait", 60_000);
    let abort = options.abort.clone();

    let run = tokio::spawn(run_provider_task(options));
    // Long enough for the thread to be open and the turn requested.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    abort.abort();

    let error = run.await.expect("the task joins").expect_err("aborted");
    assert!(error.contains("abort"), "{error}");
}

/// A resume the server refuses degrades to a fresh thread rather than failing:
/// losing continuity is a smaller harm than losing the run.
#[tokio::test]
async fn falls_back_to_a_fresh_thread_when_a_resume_is_refused() {
    let dir = TempDir::new();
    let fake = fake_app_server(&dir, TurnScript::Reply("carried on"));
    let (mut options, _) = options(&fake, &home(&dir, "resuming"), "again", 10_000);
    options.resume_session_id = Some("not-a-thread-this-server-knows".to_string());

    let result = run_provider_task(options).await.expect("the turn runs");
    assert_eq!(result.reply, "carried on");

    let methods = fake.methods();
    assert!(
        methods.contains(&"thread/resume".to_string()),
        "{methods:?}"
    );
    assert!(methods.contains(&"thread/start".to_string()), "{methods:?}");
}

/// The consent flag has to reach the thread, or a delegated task cannot commit
/// or push — the same failure the CLI seam's sandbox note describes.
#[tokio::test]
async fn carries_the_operator_consent_onto_the_thread() {
    let dir = TempDir::new();
    let fake = fake_app_server(&dir, TurnScript::Reply("ok"));
    run(&fake, &home(&dir, "consent"), "work")
        .await
        .expect("the turn runs");

    let start = fake
        .requests()
        .into_iter()
        .find(|request| request.get("method").and_then(|m| m.as_str()) == Some("thread/start"))
        .expect("a thread/start");
    let params = start.get("params").expect("params");
    assert_eq!(params["sandbox"], "danger-full-access");
    assert_eq!(params["approvalPolicy"], "never");
}

/// A consented task's approvals are granted, and the grant reaches the wire.
#[tokio::test]
async fn accepts_approvals_for_a_consented_task() {
    let dir = TempDir::new();
    let fake = fake_app_server(&dir, TurnScript::AskApproval);
    run(&fake, &home(&dir, "approving"), "work")
        .await
        .expect("the turn runs");

    let decision = fake
        .requests()
        .into_iter()
        .find(|request| {
            request
                .get("result")
                .and_then(|r| r.get("decision"))
                .is_some()
        })
        .expect("a decision was sent");
    assert_eq!(decision["result"]["decision"], "accept");
    assert_eq!(decision["id"], fake.only_ask_id());
}

/// Two lanes approving at once are answered independently.
///
/// The fake blocks each turn until its approval is answered, so a shared
/// request id would make both answers release whichever wait registered last
/// and strand the other turn until its timeout. Ids are allocated per ask for
/// exactly this reason, and this is what would catch losing that.
#[tokio::test]
async fn answers_concurrent_approvals_independently() {
    let dir = TempDir::new();
    let fake = fake_app_server(&dir, TurnScript::AskApproval);
    let home = home(&dir, "concurrent-approvals");

    let runs = (0..2).map(|index| {
        let (options, _) = options(&fake, &home, &format!("lane {index}"), 10_000);
        run_provider_task(options)
    });
    for result in futures::future::join_all(runs).await {
        result.expect("each lane runs");
    }

    let asked: Vec<serde_json::Value> = fake
        .asks()
        .into_iter()
        .map(|ask| ask["id"].clone())
        .collect();
    assert_eq!(asked.len(), 2, "both lanes asked");
    assert_ne!(asked[0], asked[1], "each ask got its own id");

    let mut answered: Vec<serde_json::Value> = fake
        .requests()
        .into_iter()
        .filter(|request| {
            request
                .get("result")
                .and_then(|r| r.get("decision"))
                .is_some()
        })
        .map(|request| request["id"].clone())
        .collect();
    answered.sort_by_key(|id| id.as_u64());
    let mut expected = asked;
    expected.sort_by_key(|id| id.as_u64());
    assert_eq!(answered, expected, "each ask was answered under its own id");
}

/// …and a task the operator did not consent to has them declined, rather than
/// prompting an operator who is not there.
#[tokio::test]
async fn declines_approvals_without_operator_consent() {
    let dir = TempDir::new();
    let fake = fake_app_server(&dir, TurnScript::AskApproval);
    let (mut options, _) = options(&fake, &home(&dir, "declining"), "work", 10_000);
    options.skip_permissions = false;

    run_provider_task(options).await.expect("the turn runs");

    let decision = fake
        .requests()
        .into_iter()
        .find(|request| {
            request
                .get("result")
                .and_then(|r| r.get("decision"))
                .is_some()
        })
        .expect("a decision was sent");
    assert_eq!(decision["result"]["decision"], "decline");
}

/// Anything the client cannot answer honestly is refused with an error, not
/// guessed at — a delegated task has no operator to ask.
#[tokio::test]
async fn refuses_a_request_it_cannot_answer() {
    let dir = TempDir::new();
    let fake = fake_app_server(&dir, TurnScript::Elicit);
    run(&fake, &home(&dir, "eliciting"), "work")
        .await
        .expect("the turn still completes");

    let refusal = fake
        .requests()
        .into_iter()
        .find(|request| request.get("error").is_some())
        .expect("a refusal was sent");
    assert_eq!(refusal["error"]["code"], -32601);
    // Correlated against the id the fake actually asked with, rather than a
    // constant: the fake allocates ids at run time so concurrent turns cannot
    // collide on one.
    assert_eq!(refusal["id"], fake.only_ask_id());
}

/// A crashed server must fail the waiting lane rather than hanging it, and the
/// pool must not hand the corpse to the next task.
#[tokio::test]
async fn replaces_a_dead_connection_on_the_next_task() {
    let dir = TempDir::new();
    let dying = fake_app_server(&dir, TurnScript::Die);
    let home = home(&dir, "dying");

    let error = run(&dying, &home, "work")
        .await
        .expect_err("the server dies mid-turn");
    assert!(error.contains("exited"), "{error}");
    assert_eq!(dying.spawn_count(), 1);

    // The next task finds the cached connection dead and starts a fresh one
    // rather than failing on a process nobody is serving.
    run(&dying, &home, "again")
        .await
        .expect_err("the replacement dies the same way");
    assert_eq!(dying.spawn_count(), 2, "the dead connection was replaced");
}

/// The pool is process-global in production, but it is an ordinary value: a
/// caller with its own can start and stop one.
#[tokio::test]
async fn a_pool_opens_one_connection_and_releases_it_on_shutdown() {
    use medulla::codex_app_server::{AppServerPool, AppServerSpec};

    let dir = TempDir::new();
    let fake = fake_app_server(&dir, TurnScript::Reply("ok"));
    let mut env = HashMap::new();
    env.insert("CODEX_HOME".to_string(), home(&dir, "pooled"));
    env.insert(
        "PATH".to_string(),
        std::env::var("PATH").unwrap_or_default(),
    );
    let spec = AppServerSpec {
        bin: fake.bin.clone(),
        args: Vec::new(),
        env,
    };

    let pool = AppServerPool::new();
    assert!(pool.is_empty().await);

    let first = pool.acquire(&spec).await.expect("opens");
    let second = pool.acquire(&spec).await.expect("reuses");
    assert!(
        Arc::ptr_eq(&first, &second),
        "the same connection came back"
    );
    assert_eq!(pool.len().await, 1);
    assert_eq!(fake.spawn_count(), 1);

    pool.shutdown().await;
    assert!(pool.is_empty().await);
}

/// A binary that is not there fails at acquisition, where the error names the
/// process, rather than on whichever task happened to be first.
#[tokio::test]
async fn reports_a_binary_that_cannot_be_spawned() {
    use medulla::codex_app_server::{AppServerPool, AppServerSpec};

    let pool = AppServerPool::new();
    let outcome = pool
        .acquire(&AppServerSpec {
            bin: "/nonexistent/codex".to_string(),
            args: Vec::new(),
            env: HashMap::new(),
        })
        .await;
    let Err(error) = outcome else {
        panic!("a binary that is not there cannot be spawned");
    };
    assert!(error.to_string().contains("/nonexistent/codex"), "{error}");
}
