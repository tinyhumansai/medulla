//! Regression coverage for the fields `PtySessionExecutor::run` used to drop
//! silently when the local host stopped selecting the headless executor:
//! `options.router`, `options.model`, `options.on_stdin`, and
//! `options.timeout_ms`. Each of these reached a spawned harness (or bounded
//! how long a turn waited) through the headless path; none of them did through
//! this one until the fixes these tests pin.

use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};

use medulla::config::{RouterConfig, RouterProviderConfig};

use super::*;

/// Build a standalone executable fake harness (not the `/bin/sh -c <script>`
/// trick the other fixtures use) so the argv this executor actually launches
/// with can be captured. The `-c script` trick works for interior shell logic
/// but cannot observe argv: `interactive_args` inserts flags (`-m <model>`)
/// *before* `extra_args`, which would land ahead of `-c` and be parsed by `sh`
/// itself rather than reaching the "harness".
///
/// The generated script records its own argv and environment to sibling files
/// before running the same rollout-writing body every other fixture here uses,
/// so a test can inspect what actually reached the child process.
fn recording_fake_harness(
    dir: &std::path::Path,
    rollout: &str,
    cwd: &str,
    reply: &str,
) -> std::path::PathBuf {
    let argv_file = dir.join("argv.txt");
    let env_file = dir.join("env.txt");
    let script = format!(
        r#"#!/bin/sh
printf '%s' "$*" > '{argv}'
env > '{envf}'
read -r prompt
printf '{{"type":"session_meta","payload":{{"session_id":"sess-fake-1","cwd":"{cwd}"}}}}\n' >> '{rollout}'
printf '{{"type":"event_msg","payload":{{"type":"task_started","turn_id":"t1"}}}}\n' >> '{rollout}'
printf '{{"type":"event_msg","payload":{{"type":"task_complete","turn_id":"t1","last_agent_message":"{reply}"}}}}\n' >> '{rollout}'
sleep 30
"#,
        argv = argv_file.to_string_lossy(),
        envf = env_file.to_string_lossy(),
    );
    let bin = dir.join("fake-harness");
    std::fs::write(&bin, script).expect("write fake harness");
    let mut perms = std::fs::metadata(&bin).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&bin, perms).expect("chmod +x");
    bin
}

/// Read a captured file, retrying briefly: the harness writes it before
/// blocking on `read`, but that write is not synchronised with the test.
fn read_captured(path: &std::path::Path) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            if !text.is_empty() || tokio::time::Instant::now() >= deadline {
                return text;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
        if tokio::time::Instant::now() >= deadline {
            return std::fs::read_to_string(path).unwrap_or_default();
        }
    }
}

#[tokio::test]
async fn the_configured_model_reaches_the_spawned_harnesss_argv() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let rollout = dir.path().join("rollout-model.jsonl");
    let bin = recording_fake_harness(dir.path(), &rollout.to_string_lossy(), &cwd, "ok");

    // Point codex at the recording script directly rather than `/bin/sh`.
    let (executor, env) = harness_with_env(
        dir.path(),
        &cwd,
        &[("TINYPLACE_CODEX_BIN", &bin.to_string_lossy())],
    );
    let mut opts = options(&env, "peer-model", "unused", &cwd);
    opts.extra_args = Vec::new();
    opts.model = Some("claude-opus-test".to_string());

    tokio::time::timeout(Duration::from_secs(30), executor.clone().run_for_test(opts))
        .await
        .expect("must settle")
        .expect("must succeed");

    let argv = read_captured(&dir.path().join("argv.txt"));
    assert_eq!(
        argv, "-m claude-opus-test",
        "the model must reach the interactive argv, matching the headless spelling"
    );
    executor.sessions_for_test().shutdown();
}

#[tokio::test]
async fn a_configured_router_layers_its_endpoint_and_resolved_key_into_the_spawn_env() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let rollout = dir.path().join("rollout-router.jsonl");
    let bin = recording_fake_harness(dir.path(), &rollout.to_string_lossy(), &cwd, "ok");

    // The daemon's own environment holds the secret; the router config carries
    // only its *name*, never the value — this is what the executor must resolve
    // at spawn.
    let (executor, env) = harness_with_env(
        dir.path(),
        &cwd,
        &[
            ("TINYPLACE_CODEX_BIN", &bin.to_string_lossy()),
            ("MY_ROUTER_KEY", "sekrit-value"),
        ],
    );

    let mut opts = options(&env, "peer-router", "unused", &cwd);
    opts.extra_args = Vec::new();
    opts.router = Some(RouterConfig {
        base_url: Some("https://top.example/v1".to_string()),
        api_key_env: Some("MY_ROUTER_KEY".to_string()),
        providers: std::collections::HashMap::from([(
            "codex".to_string(),
            RouterProviderConfig {
                base_url: Some("https://codex.example/v1".to_string()),
            },
        )]),
        ..Default::default()
    });

    tokio::time::timeout(Duration::from_secs(30), executor.clone().run_for_test(opts))
        .await
        .expect("must settle")
        .expect("must succeed");

    let spawned_env = read_captured(&dir.path().join("env.txt"));
    assert!(
        spawned_env.contains("OPENAI_BASE_URL=https://codex.example/v1"),
        "the provider-scoped baseUrl must win over the top-level one: {spawned_env}"
    );
    assert!(
        spawned_env.contains("OPENAI_API_KEY=sekrit-value"),
        "the api key must be resolved BY NAME from the executor's own env, never inlined \
         in config: {spawned_env}"
    );
    executor.sessions_for_test().shutdown();
}

#[tokio::test]
async fn task_scoped_environment_reaches_the_spawned_harness() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let rollout = dir.path().join("rollout-task-env.jsonl");
    let bin = recording_fake_harness(dir.path(), &rollout.to_string_lossy(), &cwd, "ok");
    let (executor, env) = harness_with_env(
        dir.path(),
        &cwd,
        &[("TINYPLACE_CODEX_BIN", &bin.to_string_lossy())],
    );
    let mut opts = options(&env, "peer-task-env", "unused", &cwd);
    opts.extra_args = Vec::new();
    opts.env.insert(
        "ANTHROPIC_DEFAULT_OPUS_MODEL".into(),
        "openrouter/task-model".into(),
    );

    tokio::time::timeout(Duration::from_secs(30), executor.clone().run_for_test(opts))
        .await
        .expect("must settle")
        .expect("must succeed");

    let spawned_env = read_captured(&dir.path().join("env.txt"));
    assert!(
        spawned_env.contains("ANTHROPIC_DEFAULT_OPUS_MODEL=openrouter/task-model"),
        "task-scoped harness overrides must reach the child: {spawned_env}"
    );
    executor.sessions_for_test().shutdown();
}

#[tokio::test]
async fn a_router_key_env_that_is_not_set_fails_the_turn_before_it_ever_spawns() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let (executor, env) = harness(dir.path(), &cwd);

    let mut opts = options(&env, "peer-missing-key", "true", &cwd);
    opts.router = Some(RouterConfig {
        base_url: Some("https://top.example/v1".to_string()),
        api_key_env: Some("NEVER_SET_IN_THIS_ENV".to_string()),
        ..Default::default()
    });

    let error = executor
        .clone()
        .run_for_test(opts)
        .await
        .expect_err("an unset key env must be a hard error, never a silent empty key");
    assert!(
        error.contains("NEVER_SET_IN_THIS_ENV") && error.contains("is not set"),
        "got: {error}"
    );
}

#[tokio::test]
async fn a_configured_idle_timeout_shorter_than_the_fixed_budgets_is_honored() {
    // A harness that reads the prompt and then goes silent forever. The fixed
    // LOCATE_BUDGET (30s) would eventually fail this turn too, but with a
    // different message and thirty real seconds later — this pins that the
    // *configured* ceiling is checked first and actually bounds the wait.
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let script = "read -r prompt; sleep 30".to_string();

    let (executor, env) = harness(dir.path(), &cwd);
    let mut opts = options(&env, "peer-timeout", &script, &cwd);
    opts.timeout_ms = 300;

    let error = tokio::time::timeout(Duration::from_secs(10), executor.clone().run_for_test(opts))
        .await
        .expect("the configured timeout must fire well inside the fixed 30s locate budget")
        .expect_err("an idle harness must fail the turn, not hang until the fixed budget");

    assert!(
        error.contains("idle for 300ms"),
        "must name the configured ceiling it hit, not the fixed one: {error}"
    );
    executor.sessions_for_test().shutdown();
}

#[tokio::test]
async fn stdin_frames_are_typed_into_the_running_harness() {
    // The dispatcher's `TaskFrameKind::Input` reaches a running local task
    // through `RunTaskOptions::on_stdin`: a registration callback the executor
    // must call with a sender, then drain into the harness. Left unwired, the
    // registration never happens and every mid-turn steering message from a
    // peer is silently dropped.
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let rollout = dir.path().join("rollout-stdin.jsonl");
    // Replies only once a *second* line arrives, so completion is proof the
    // injected text reached the child's stdin/composer rather than merely the
    // initial prompt.
    let script = format!(
        r#"
read -r prompt
printf '{{"type":"session_meta","payload":{{"session_id":"sess-fake-1","cwd":"{cwd}"}}}}\n' >> '{rollout}'
printf '{{"type":"event_msg","payload":{{"type":"task_started","turn_id":"t1"}}}}\n' >> '{rollout}'
read -r followup
printf '{{"type":"event_msg","payload":{{"type":"task_complete","turn_id":"t1","last_agent_message":"got:'"$followup"'"}}}}\n' >> '{rollout}'
sleep 30
"#,
        rollout = rollout.to_string_lossy(),
        cwd = cwd,
    );

    let (executor, env) = harness(dir.path(), &cwd);
    let mut opts = options(&env, "peer-stdin", &script, &cwd);
    let registered: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedSender<String>>>> =
        Arc::new(Mutex::new(None));
    let registered_for_cb = registered.clone();
    opts.on_stdin = Some(Box::new(move |tx| {
        *registered_for_cb.lock().unwrap() = Some(tx);
    }));

    let handle = tokio::spawn({
        let executor = executor.clone();
        async move { executor.run_for_test(opts).await }
    });

    // Wait for the registration (set synchronously once the session opens) and
    // for the harness to have reached its second `read`, evidenced by
    // `task_started` in the transcript.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let has_sender = registered.lock().unwrap().is_some();
        let started = std::fs::read_to_string(&rollout)
            .map(|t| t.contains("task_started"))
            .unwrap_or(false);
        if has_sender && started {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "on_stdin was never registered, or the harness never reached its second read"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    registered
        .lock()
        .unwrap()
        .take()
        .expect("registered above")
        .send("steer-here".to_string())
        .expect("the drain task is still alive");

    let result = tokio::time::timeout(Duration::from_secs(15), handle)
        .await
        .expect("must settle")
        .expect("task did not panic")
        .expect("must succeed");

    assert_eq!(
        result.reply, "got:steer-here",
        "the reply proves the injected text reached the harness's stdin, not just the prompt"
    );
    executor.sessions_for_test().shutdown();
}

#[tokio::test]
async fn a_timed_out_turn_stops_its_harness_instead_of_leaving_it_running() {
    // Honouring `taskTimeoutMs` without stopping the child is worse than not
    // honouring it: the peer is told the task failed while the harness keeps
    // editing the workspace, and an unbound session is then released as idle so
    // the *next* task claims a harness that is still mid-turn and pastes its
    // prompt into the same composer.
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let (executor, env) = harness(dir.path(), &cwd);
    let sessions = executor.sessions_for_test();

    // A harness that starts, greets the pty so the session is live, and then
    // says nothing further — silence on the transcript with a child very much
    // alive, which is exactly what the idle watchdog is for.
    let script = "printf 'ready\\n'; sleep 600";
    // Named sender + unbound: the case where the session would otherwise be
    // handed straight to the next turn.
    let mut options = options(&env, "peerA", script, &cwd);
    options.session_class = medulla::sessions::SessionClass::Unbound;
    options.timeout_ms = 1_500;

    let error = tokio::time::timeout(
        Duration::from_secs(30),
        executor.clone().run_for_test(options),
    )
    .await
    .expect("the watchdog must fire well inside this budget")
    .expect_err("a turn that never produces a transcript line is a failure");
    assert!(
        error.contains("idle for 1500ms"),
        "the configured ceiling should be what fired: {error}"
    );

    // The point of the fix: nothing is left running for the next task to claim.
    let rows = sessions.rows();
    assert!(
        !rows.iter().any(|row| row.state.is_running()),
        "a timed-out turn must not leave its harness alive: {:?}",
        rows.iter().map(|r| r.state).collect::<Vec<_>>()
    );
    assert!(
        sessions
            .claim_idle("peerA", medulla::tinyplace::HarnessProvider::Codex)
            .is_none(),
        "a timed-out session must not be reusable by the next task"
    );
    sessions.shutdown();
}

/// The watchable PTY path spawns the harness itself rather than going through
/// the headless seam, so it has to apply attribution on its own — without this
/// the default visible execution path produced unattributed commits even with
/// `attribution.commit = true`.
#[tokio::test]
async fn the_pty_spawn_env_carries_commit_attribution() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let rollout = dir.path().join("rollout-attribution.jsonl");
    let bin = recording_fake_harness(dir.path(), &rollout.to_string_lossy(), &cwd, "ok");

    let (executor, env) = harness_with_env(
        dir.path(),
        &cwd,
        &[("TINYPLACE_CODEX_BIN", &bin.to_string_lossy())],
    );

    let mut opts = options(&env, "peer-attribution", "unused", &cwd);
    opts.extra_args = Vec::new();
    opts.attribution = true;

    tokio::time::timeout(Duration::from_secs(30), executor.clone().run_for_test(opts))
        .await
        .expect("must settle")
        .expect("must succeed");

    let spawned_env = read_captured(&dir.path().join("env.txt"));
    assert!(
        spawned_env.contains("MEDULLA_ATTRIBUTION=Co-authored-by: Medulla"),
        "the PTY child must carry the attribution trailer: {spawned_env}"
    );
    assert!(
        spawned_env.contains("GIT_CONFIG_KEY_0=core.hooksPath"),
        "the PTY child must have the hook directory activated: {spawned_env}"
    );
    executor.sessions_for_test().shutdown();
}

/// And it must stay off when config says so.
#[tokio::test]
async fn the_pty_spawn_env_omits_attribution_when_configured_off() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let rollout = dir.path().join("rollout-no-attribution.jsonl");
    let bin = recording_fake_harness(dir.path(), &rollout.to_string_lossy(), &cwd, "ok");

    let (executor, env) = harness_with_env(
        dir.path(),
        &cwd,
        &[("TINYPLACE_CODEX_BIN", &bin.to_string_lossy())],
    );

    let mut opts = options(&env, "peer-no-attribution", "unused", &cwd);
    opts.extra_args = Vec::new();
    opts.attribution = false;

    tokio::time::timeout(Duration::from_secs(30), executor.clone().run_for_test(opts))
        .await
        .expect("must settle")
        .expect("must succeed");

    let spawned_env = read_captured(&dir.path().join("env.txt"));
    assert!(
        !spawned_env.contains("MEDULLA_ATTRIBUTION"),
        "attribution must be absent when turned off: {spawned_env}"
    );
    executor.sessions_for_test().shutdown();
}
