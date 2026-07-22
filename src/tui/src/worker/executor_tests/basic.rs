//! One task in one session: run, bound/unattributed, refusal, abort, adapter, and error paths.

use super::*;

#[tokio::test]
async fn a_task_runs_in_a_live_session_and_returns_what_the_harness_said() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let rollout = dir.path().join("rollout-fake.jsonl");
    let script = fake_harness_script(&rollout.to_string_lossy(), &cwd, "shipped it");

    let (executor, env) = harness(dir.path(), &cwd);
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        executor
            .clone()
            .run_for_test(options(&env, "peer-alice", &script, &cwd)),
    )
    .await
    .expect("the turn must settle, not hang")
    .expect("the turn must succeed");

    assert_eq!(
        result.reply, "shipped it",
        "the reply is what the harness stated, read from its own transcript"
    );
    assert_eq!(result.provider, HarnessProvider::Codex);
}

#[tokio::test]
async fn an_unattributed_task_is_bounded_and_leaves_no_session_behind() {
    // No sender means discrete work: one session, one turn, gone on reply.
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let rollout = dir.path().join("rollout-bounded.jsonl");
    let script = fake_harness_script(&rollout.to_string_lossy(), &cwd, "done");

    let (executor, env) = harness(dir.path(), &cwd);
    let sessions = executor.sessions_for_test();
    tokio::time::timeout(
        Duration::from_secs(30),
        executor
            .clone()
            .run_for_test(options(&env, "", &script, &cwd)),
    )
    .await
    .expect("must settle")
    .expect("must succeed");

    assert!(
        sessions.rows().iter().all(|row| !row.state.is_running()),
        "a bounded task must not leave a live session behind"
    );
}

#[tokio::test]
async fn opencode_is_refused_rather_than_left_to_hang() {
    // It writes no transcript this can read, so a turn on it could never be
    // known to have finished. Failing fast beats stalling the peer.
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let (executor, env) = harness(dir.path(), &cwd);
    let mut opts = options(&env, "peer", "true", &cwd);
    opts.provider = HarnessProvider::Opencode;

    let error = executor
        .clone()
        .run_for_test(opts)
        .await
        .expect_err("opencode must be refused");
    assert!(error.contains("watchable"), "got: {error}");
}

#[tokio::test]
async fn an_aborted_task_stops_waiting_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    // A harness that never completes its turn.
    let script = "read -r prompt; sleep 30".to_string();

    let (executor, env) = harness(dir.path(), &cwd);
    let mut opts = options(&env, "peer", &script, &cwd);
    let abort = opts.abort.clone();
    opts.abort = abort.clone();

    let handle = tokio::spawn({
        let executor = executor.clone();
        async move { executor.run_for_test(opts).await }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    abort.abort();

    let result = tokio::time::timeout(Duration::from_secs(30), handle)
        .await
        .expect("abort must be observed promptly")
        .expect("task did not panic");
    let error = result.expect_err("an aborted turn is an error, not a silent success");
    assert!(error.contains("aborted"), "got: {error}");
    executor.sessions_for_test().shutdown();
}

#[tokio::test]
async fn the_run_task_adapter_runs_a_turn_like_the_daemon_would() {
    // `into_run_task` is the seam the daemon runtime actually calls. Exercising
    // it, rather than only `run_for_test`, proves the adapter forwards options
    // and awaits the same body — a broken adapter would strand every task.
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let rollout = dir.path().join("rollout-adapter.jsonl");
    let script = fake_harness_script(&rollout.to_string_lossy(), &cwd, "adapter reply");

    let (executor, env) = harness(dir.path(), &cwd);
    let run_task = executor.clone().into_run_task();
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        run_task(options(&env, "", &script, &cwd)),
    )
    .await
    .expect("the turn must settle, not hang")
    .expect("the turn must succeed");

    assert_eq!(result.reply, "adapter reply");
    assert_eq!(result.provider, HarnessProvider::Codex);
}

#[tokio::test]
async fn a_session_that_exits_before_replying_is_an_error_not_a_hang() {
    // The harness reads the prompt and dies without ever writing a completion.
    // The turn must fail promptly with a diagnosis, not spin until its timeout
    // waiting for a transcript that will never grow.
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    // No rollout written, and the process exits right after reading the prompt.
    let script = "read -r prompt; exit 0".to_string();

    let (executor, env) = harness(dir.path(), &cwd);
    let error = tokio::time::timeout(
        Duration::from_secs(30),
        executor
            .clone()
            .run_for_test(options(&env, "peer-eve", &script, &cwd)),
    )
    .await
    .expect("a dead session must be noticed promptly, not waited out")
    .expect_err("a session that exits mid-turn is an error");
    assert!(
        error.contains("ended before the turn did"),
        "the error must name the cause; got: {error}"
    );
    executor.sessions_for_test().shutdown();
}

/// A fake harness that paints a startup trust dialog and then blocks reading —
/// exactly the shape `blocking_dialog` refuses to type into. Injection fails on
/// it, which is how the executor's prompt-injection error path is reached.

#[test]
fn only_transcript_writing_harnesses_can_run_watchable_tasks() {
    use medulla::session_history::SessionAgentKind;

    // claude and codex write flat transcripts this executor can tail; opencode
    // does not, so a turn on it could never be known to have finished and it is
    // refused rather than run.
    assert_eq!(
        super::super::executor::agent_kind(HarnessProvider::Claude),
        Some(SessionAgentKind::Claude)
    );
    assert_eq!(
        super::super::executor::agent_kind(HarnessProvider::Codex),
        Some(SessionAgentKind::Codex)
    );
    assert_eq!(
        super::super::executor::agent_kind(HarnessProvider::Opencode),
        None
    );
}
