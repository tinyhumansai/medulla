//! End-to-end tests for [`PtySessionExecutor`].
//!
//! These use a **fake harness**: a shell script that paints to its pty (so the
//! session is watchable, exactly like a real one) and appends records to a
//! transcript in the codex rollout dialect. That exercises the whole path —
//! spawn, inject, tail, fold, settle — with no coding agent installed and no
//! network, which is what makes it deterministic.
//!
//! Codex's dialect is used because it takes no preset session id, so the script
//! can own its transcript filename. The claude path differs only in which
//! records mean "done", and that fold is pinned in the SDK's own tests.

use std::collections::HashMap;
use std::time::Duration;

use medulla::daemon::providers::{Abort, RunTaskOptions};
use medulla::tinyplace::HarnessProvider;

use super::executor::PtySessionExecutor;
use super::pty::PtyManager;

/// A shell script standing in for a coding agent.
///
/// It reads the injected prompt from its pty, echoes it (so the pane shows
/// something), then writes a rollout that says the turn is complete.
fn fake_harness_script(rollout: &str, cwd: &str, reply: &str) -> String {
    fake_harness_script_as(rollout, cwd, reply, "sess-fake-1")
}

/// As [`fake_harness_script`], with the harness session id stated.
///
/// Concurrent sessions must each claim their own: the tailer pins to the id it
/// learns from the rollout, so two sessions reporting the same one are
/// indistinguishable to it and both tails can settle on whichever rollout is
/// found first. Real codex sessions mint distinct ids; a fixture that does not
/// was testing something the product never sees.
fn fake_harness_script_as(rollout: &str, cwd: &str, reply: &str, session_id: &str) -> String {
    format!(
        r#"
read -r prompt
printf 'working on: %s\r\n' "$prompt"
printf '{{"type":"session_meta","payload":{{"session_id":"{session_id}","cwd":"{cwd}"}}}}\n' >> '{rollout}'
printf '{{"type":"event_msg","payload":{{"type":"task_started","turn_id":"t1"}}}}\n' >> '{rollout}'
printf '{{"type":"event_msg","payload":{{"type":"agent_message","message":"looking at it","phase":"main"}}}}\n' >> '{rollout}'
printf '{{"type":"event_msg","payload":{{"type":"task_complete","turn_id":"t1","last_agent_message":"{reply}"}}}}\n' >> '{rollout}'
sleep 30
"#
    )
}

/// Build an executor whose sessions run `script` instead of a real harness.
fn harness(
    sessions_dir: &std::path::Path,
    workspace: &str,
) -> (PtySessionExecutor, HashMap<String, String>) {
    let mut env = HashMap::new();
    env.insert(
        "PATH".to_string(),
        std::env::var("PATH").unwrap_or_default(),
    );
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    // Point transcript discovery at the temp dir, and the "codex" binary at sh.
    env.insert(
        "TINYPLACE_CODEX_SESSIONS_DIR".to_string(),
        sessions_dir.to_string_lossy().into_owned(),
    );
    env.insert("TINYPLACE_CODEX_BIN".to_string(), "/bin/sh".to_string());
    let executor = PtySessionExecutor::new(PtyManager::new(), env.clone(), workspace.to_string());
    (executor, env)
}

fn options(
    env: &HashMap<String, String>,
    conversation: &str,
    script: &str,
    cwd: &str,
) -> RunTaskOptions {
    RunTaskOptions {
        conversation: conversation.to_string(),
        resume_session_id: None,
        provider: HarnessProvider::Codex,
        prompt: "ship the fix".to_string(),
        cwd: cwd.to_string(),
        env: env.clone(),
        timeout_ms: 30_000,
        model: None,
        agent: None,
        // The script *is* the fake harness; `-c` makes sh run it.
        extra_args: vec!["-c".to_string(), script.to_string()],
        skip_permissions: false,
        abort: Abort::new(),
        on_event: None,
        on_stdin: None,
    }
}

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
async fn a_peers_conversation_reuses_its_own_session() {
    // Unbound: the peer's next message continues the same session, which is what
    // makes a conversation remember.
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let rollout = dir.path().join("rollout-conv.jsonl");
    let script = fake_harness_script(&rollout.to_string_lossy(), &cwd, "first");

    let (executor, env) = harness(dir.path(), &cwd);
    let sessions = executor.sessions_for_test();

    tokio::time::timeout(
        Duration::from_secs(30),
        executor
            .clone()
            .run_for_test(options(&env, "peer-bob", &script, &cwd)),
    )
    .await
    .expect("must settle")
    .expect("must succeed");

    let after_first: Vec<String> = sessions.rows().iter().map(|r| r.id.clone()).collect();
    assert_eq!(after_first.len(), 1);
    assert!(
        sessions.rows()[0].state.is_running(),
        "an unbound session stays alive for the peer's next message"
    );
    assert_eq!(sessions.rows()[0].label, "peer-bob");
    sessions.shutdown();
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
async fn a_peer_is_shown_progress_while_its_task_runs() {
    // Regression. This executor used to drop the daemon's `on_event` callback
    // entirely, so a peer dispatching work in --tui mode got an ack, then
    // silence, then a reply — while the headless daemon streamed status frames
    // throughout. Both modes now fold through the same shared `TurnStream`, so
    // they cannot report differently.
    use std::sync::{Arc, Mutex};

    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let rollout = dir.path().join("rollout-progress.jsonl");
    let script = fake_harness_script(&rollout.to_string_lossy(), &cwd, "all done");

    let (executor, env) = harness(dir.path(), &cwd);
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let mut opts = options(&env, "peer-alice", &script, &cwd);
    opts.on_event = Some({
        let seen = seen.clone();
        Box::new(
            move |event: &medulla::daemon::mappers::HarnessSemanticEvent| {
                seen.lock().unwrap().push(event.event.kind.clone());
            },
        )
    });

    let result = tokio::time::timeout(Duration::from_secs(30), executor.clone().run_for_test(opts))
        .await
        .expect("must settle")
        .expect("must succeed");

    assert_eq!(result.reply, "all done");
    let kinds = seen.lock().unwrap().clone();
    assert!(
        kinds.iter().any(|k| k == "agent_message"),
        "the peer must see progress, not just the final reply; got {kinds:?}"
    );
    assert!(
        result.events > 0,
        "events are what the daemon throttles into status frames"
    );
}

/// A harness that answers repeatedly on one long-lived session, as a real one
/// does: it reads a prompt, appends a completed turn to the *same* rollout, and
/// waits for the next.
fn conversational_harness_script(rollout: &str, cwd: &str) -> String {
    format!(
        r#"
printf '{{"type":"session_meta","payload":{{"session_id":"sess-fake-1","cwd":"{cwd}"}}}}\n' >> '{rollout}'
turn=0
while read -r prompt; do
  turn=$((turn+1))
  printf 'working on: %s\r\n' "$prompt"
  printf '{{"type":"event_msg","payload":{{"type":"task_started","turn_id":"t'$turn'"}}}}\n' >> '{rollout}'
  printf '{{"type":"event_msg","payload":{{"type":"task_complete","turn_id":"t'$turn'","last_agent_message":"answer '$turn'"}}}}\n' >> '{rollout}'
done
"#
    )
}

#[tokio::test]
async fn a_second_turn_on_a_reused_session_gets_its_own_answer() {
    // Regression, seen in the field: the first task on a peer's session
    // succeeded and the reply reached the orchestrator; the next one died with
    // "claude never started a turn".
    //
    // The tailer is built for a session about to start — it ignores every
    // transcript that already exists and discounts any file older than the
    // launch instant. A reused session's transcript is both, so it could never
    // be located again. Worse, had it been located at byte zero, the previous
    // turn's completion record is still in the file: the fold would settle on it
    // at once and hand the peer the answer to its *previous* question.
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let rollout = dir.path().join("rollout-conversation.jsonl");
    let script = conversational_harness_script(&rollout.to_string_lossy(), &cwd);

    let (executor, env) = harness(dir.path(), &cwd);
    let sessions = executor.sessions_for_test();

    let first = tokio::time::timeout(
        Duration::from_secs(30),
        executor
            .clone()
            .run_for_test(options(&env, "peer-carol", &script, &cwd)),
    )
    .await
    .expect("the first turn must settle")
    .expect("the first turn must succeed");
    assert_eq!(first.reply, "answer 1");
    assert_eq!(sessions.rows().len(), 1, "one session for the conversation");
    // The harness id has to be recorded on the way through, or there is nothing
    // to resume by and the next turn silently falls back to fresh-session
    // discovery — which cannot find a transcript that already existed.
    assert_eq!(
        sessions.rows()[0].session_id.as_deref(),
        Some("sess-fake-1"),
        "the session must carry the harness id after its first turn"
    );

    // Same peer, same session — the case that failed.
    let second = tokio::time::timeout(
        Duration::from_secs(60),
        executor
            .clone()
            .run_for_test(options(&env, "peer-carol", &script, &cwd)),
    )
    .await
    .expect("a second turn on a live session must settle, not time out")
    .expect("a second turn on a live session must succeed");

    assert_eq!(
        second.reply, "answer 2",
        "the peer must get this turn's answer, not the previous one"
    );
    assert_eq!(
        sessions.rows().len(),
        1,
        "the conversation stayed on one session"
    );
    sessions.shutdown();
}

#[tokio::test]
async fn concurrent_tasks_from_one_peer_do_not_share_a_session() {
    // Reproduced from the field. The orchestrator fans out — its prompt calls
    // delegation its DEFAULT path — so three tasks arrived from the same peer at
    // once. All three found the same running session, were pasted into the same
    // composer, and all three tails settled on the same completion:
    //
    //   repo-scan-1              reply · 872 chars: Continuing from the scan I already ran…
    //   repo-scan-2              reply · 872 chars: Continuing from the scan I already ran…
    //   repo-scan-claude-daemon  reply · 872 chars: Continuing from the scan I already ran…
    //
    // Three different instructions, one answer, delivered three times.
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let (executor, env) = harness(dir.path(), &cwd);
    let sessions = executor.sessions_for_test();

    // Each session writes its own rollout, so a shared session would be visible
    // as a shared transcript rather than only as a shared answer.
    let script = |n: u32| {
        let rollout = dir.path().join(format!("rollout-{n}.jsonl"));
        fake_harness_script_as(
            &rollout.to_string_lossy(),
            &cwd,
            &format!("answer {n}"),
            &format!("sess-fake-{n}"),
        )
    };

    let a = tokio::spawn({
        let (e, env, s) = (executor.clone(), env.clone(), script(1));
        async move {
            e.run_for_test(options(&env, "peer-dave", &s, &cwd_of(&env)))
                .await
        }
    });
    let b = tokio::spawn({
        let (e, env, s) = (executor.clone(), env.clone(), script(2));
        async move {
            e.run_for_test(options(&env, "peer-dave", &s, &cwd_of(&env)))
                .await
        }
    });

    let (ra, rb) = tokio::join!(a, b);
    let ra = ra.expect("no panic").expect("first task must succeed");
    let rb = rb.expect("no panic").expect("second task must succeed");

    assert_ne!(
        ra.reply, rb.reply,
        "two concurrent tasks must get their own answers, not one answer twice"
    );
    assert_eq!(
        sessions.rows().len(),
        2,
        "a busy session must not be reused; the second task needs its own"
    );
    sessions.shutdown();
}

/// The workspace an options set was built for.
fn cwd_of(env: &HashMap<String, String>) -> String {
    env.get("TINYPLACE_CODEX_SESSIONS_DIR")
        .cloned()
        .unwrap_or_default()
}
