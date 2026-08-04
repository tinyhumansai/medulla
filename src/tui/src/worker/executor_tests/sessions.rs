//! Session reuse, concurrency, and the prompt-injection failure paths.

use super::*;

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

#[tokio::test]
async fn a_bounded_task_whose_prompt_cannot_be_injected_closes_its_session() {
    // The harness is stuck on a modal, so the prompt never lands and the turn is
    // an error rather than a hang. A bounded task's session must be *closed* on
    // that error, not left running forever.
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let (executor, env) = harness(dir.path(), &cwd);
    let sessions = executor.sessions_for_test();

    // Empty conversation ⇒ bounded work.
    let error = tokio::time::timeout(
        Duration::from_secs(30),
        executor
            .clone()
            .run_for_test(options(&env, "", &dialog_harness_script(), &cwd)),
    )
    .await
    .expect("a blocked injection must fail promptly, not hang")
    .expect_err("a prompt that cannot be injected is an error");
    assert!(
        error.contains("approve this workspace"),
        "the error must name what is in the way: {error}"
    );
    assert!(
        sessions.rows().iter().all(|row| !row.state.is_running()),
        "a bounded task must close its session when injection fails"
    );
    sessions.shutdown();
}

#[tokio::test]
async fn an_unbound_task_whose_prompt_cannot_be_injected_releases_its_session() {
    // The conversational case: the session stays alive for the peer's next turn,
    // but it must be *released* (marked idle) so a later task can reuse it — a
    // session left claimed by a failed turn would never be reusable again.
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let (executor, env) = harness(dir.path(), &cwd);
    let sessions = executor.sessions_for_test();

    let error = tokio::time::timeout(
        Duration::from_secs(30),
        executor
            .clone()
            .run_for_test(options(&env, "peer-frank", &dialog_harness_script(), &cwd)),
    )
    .await
    .expect("a blocked injection must fail promptly, not hang")
    .expect_err("a prompt that cannot be injected is an error");
    assert!(error.contains("approve this workspace"), "got: {error}");

    let rows = sessions.rows();
    assert_eq!(
        rows.len(),
        1,
        "the unbound session survives the failed turn"
    );
    assert!(
        rows[0].state.is_running(),
        "an unbound session stays alive for the next message"
    );
    assert!(
        !rows[0].busy,
        "a failed turn must release its session so it can be reused"
    );
    sessions.shutdown();
}

#[tokio::test]
async fn sequential_task_frames_from_one_sender_do_not_share_a_session() {
    // The regression this pins is not the concurrent case above — that one was
    // already handled by refusing a *busy* session. This is the sequential one:
    // task A finishes, its session is released as idle, and task B then claims
    // it and inherits A's whole conversation.
    //
    // It was reachable for every locally dispatched task. `DaemonRuntime` sets
    // `conversation` to the authenticated sender, which for an orchestrator's
    // own host is always the hub address, so the old
    // `conversation.is_empty() => Bounded` inference never fired: every task
    // frame classified as unbound and reused the sender's session. Two
    // unrelated delegated tasks then ran in one harness, the second able to
    // read and act on the first's prompt and tool output.
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let (executor, env) = harness(dir.path(), &cwd);
    let sessions = executor.sessions_for_test();

    // Each session writes its own rollout, so sharing shows up as a shared
    // transcript rather than only as a suspicious answer.
    let task = |n: u32| {
        let rollout = dir.path().join(format!("rollout-seq-{n}.jsonl"));
        let script = fake_harness_script_as(
            &rollout.to_string_lossy(),
            &cwd,
            &format!("answer {n}"),
            &format!("sess-seq-{n}"),
        );
        // A named sender, exactly as the daemon supplies — and `Bounded`,
        // because a task frame is discrete work however it is attributed. That
        // pairing is the one the old inference could not express.
        let mut options = options(&env, "peerA", &script, &cwd);
        options.session_class = medulla::sessions::SessionClass::Bounded;
        options
    };

    let first = tokio::time::timeout(
        Duration::from_secs(30),
        executor.clone().run_for_test(task(1)),
    )
    .await
    .expect("the first task settles")
    .expect("the first task must succeed");

    // Sequential, not concurrent: the first session is idle and reusable by the
    // time the second starts, which is precisely the window that leaked.
    let second = tokio::time::timeout(
        Duration::from_secs(30),
        executor.clone().run_for_test(task(2)),
    )
    .await
    .expect("the second task settles")
    .expect("the second task must succeed");

    assert_ne!(
        first.reply, second.reply,
        "each task must be answered on its own terms, not handed the other's answer"
    );
    // Two rows, not one. A closed session keeps its row so its last screen
    // stays readable, so this counts sessions *opened*: one each. With the old
    // inference the second task reclaimed the first's idle session and this was
    // 1 — the leak, stated as a number.
    let rows = sessions.rows();
    assert_eq!(
        rows.len(),
        2,
        "each bounded task must open its own session, not inherit the last one"
    );
    assert!(
        !rows.iter().any(|row| row.state.is_running()),
        "a bounded task closes its session when it replies: {:?}",
        rows.iter().map(|r| r.state).collect::<Vec<_>>()
    );
    sessions.shutdown();
}

#[tokio::test]
async fn a_dispatch_into_a_workspace_the_operator_holds_is_refused() {
    // The bug this closes: taking over the only harness in a workspace did not
    // stop the orchestrator working there — `session_for` fell through the reuse
    // branch and simply OPENED A SECOND HARNESS in the same folder. Two agents,
    // one working tree, no mutual exclusion. So the assertion that matters here
    // is not just that the task is refused; it is that nothing new was spawned.
    use super::super::pty::{HarnessControl, LaunchSpec};

    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let rollout = dir.path().join("rollout-held.jsonl");
    let script = fake_harness_script(&rollout.to_string_lossy(), &cwd, "unreachable");

    let (executor, env) = harness(dir.path(), &cwd);
    let sessions = executor.sessions_for_test();

    // The operator starts a harness here and keeps it.
    let held = sessions
        .open(LaunchSpec {
            provider: HarnessProvider::Codex,
            bin: "/bin/sh".to_string(),
            cwd: cwd.clone(),
            env: HashMap::new(),
            extra_args: vec!["-c".to_string(), "sleep 30".to_string()],
            skip_permissions: false,
            label: "you:codex".to_string(),
            model: None,
            session_id: None,
            control: HarnessControl::User,
            user_spawned: true,
            mcp_grant_session: None,
        })
        .expect("the operator's harness must start");
    let before = sessions.rows().len();

    let error = tokio::time::timeout(
        Duration::from_secs(30),
        executor
            .clone()
            .run_for_test(options(&env, "peer-bob", &script, &cwd)),
    )
    .await
    .expect("must settle")
    .expect_err("a workspace an operator holds must refuse the task");

    assert!(
        error.starts_with(medulla::daemon::HARNESS_HELD_PREFIX),
        "the refusal must carry the shared prefix so the hub can settle it as \
         Held rather than as an ordinary worker error — got: {error}"
    );
    assert_eq!(
        sessions.rows().len(),
        before,
        "refusing the task must not leave a rival harness running in the \
         operator's working tree"
    );

    sessions.close(&held);
    sessions.shutdown();
}

#[tokio::test]
async fn a_dispatch_runs_again_once_the_harness_is_handed_back() {
    // The other half: the hold is a pause, not a wall. Handing back must make
    // the workspace usable again without the operator restarting anything.
    use super::super::pty::{HarnessControl, LaunchSpec};

    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let rollout = dir.path().join("rollout-handback.jsonl");
    let script = fake_harness_script(&rollout.to_string_lossy(), &cwd, "picked it up");

    let (executor, env) = harness(dir.path(), &cwd);
    let sessions = executor.sessions_for_test();

    // The operator's harness runs the fake agent, not a bare shell: after the
    // handback the executor REUSES this very process, so it has to be something
    // that can actually serve the turn.
    let held = sessions
        .open(LaunchSpec {
            provider: HarnessProvider::Codex,
            bin: "/bin/sh".to_string(),
            cwd: cwd.clone(),
            env: env.clone(),
            extra_args: vec!["-c".to_string(), script.clone()],
            skip_permissions: false,
            label: "you:codex".to_string(),
            model: None,
            session_id: None,
            control: HarnessControl::User,
            user_spawned: true,
            mcp_grant_session: None,
        })
        .expect("the operator's harness must start");

    assert!(tokio::time::timeout(
        Duration::from_secs(30),
        executor
            .clone()
            .run_for_test(options(&env, "peer-bob", &script, &cwd)),
    )
    .await
    .expect("must settle")
    .is_err());

    // The operator hands it back.
    assert!(sessions.set_control(&held, HarnessControl::Orchestrator));

    let reply = tokio::time::timeout(
        Duration::from_secs(30),
        executor
            .clone()
            .run_for_test(options(&env, "peer-bob", &script, &cwd)),
    )
    .await
    .expect("must settle")
    .expect("a handed-back workspace must accept work again");
    assert!(reply.reply.contains("picked it up"), "got: {reply:?}");

    sessions.shutdown();
}
