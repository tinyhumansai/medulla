//! What a dispatch does when it meets a person: candidacy, the queue, and the
//! suspend / hand-back cycle.
//!
//! The whole of phase E's control model, exercised on one machine against the
//! fake harness. Each test pins one clause of it, and the clauses are worth
//! naming because the behaviour they replace was the opposite in every case:
//!
//! | Was | Is |
//! |---|---|
//! | a hold on one session refused the whole workspace | a held session is simply not a candidate; a sibling serves the task |
//! | nothing else available ⇒ the dispatch failed `harnessHeld` | it queues, and runs when the session comes back |
//! | a mid-turn takeover discarded the in-flight turn | the turn suspends and keeps everything it had |
//! | a held task hit the idle watchdog | held time does not accrue |
//! | a held task produced no result, ever | the hand-back turn produces one, under the original task id |
//!
//! The last row is why these are the deliverable rather than the safety net:
//! control state is no longer advertised to the backend at all, so the hand-back
//! turn is now the *only* way a held in-flight task reaches a result. A silent
//! dispatch would be an orchestrator waiting forever on a task it cannot see the
//! state of.
//!
//! These replace `a_dispatch_into_a_workspace_the_operator_holds_is_refused` and
//! `a_dispatch_runs_again_once_the_harness_is_handed_back`, which pinned the
//! refusal — the invariant those really protected (never a rival harness in the
//! operator's tree) is asserted here on the queue path instead.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use medulla::protocol::HarnessProvider;

use super::super::pty::{LaunchSpec, PtyManager, SessionControl, SessionOrigin};
use super::{conversational_harness_script, fake_harness_script, harness, options};

/// How long to let a suspended or queued dispatch prove it is *not* finishing.
///
/// Long enough to outlast the executor's own 250 ms hold poll several times
/// over, short enough that four of these do not dominate the suite.
const NOT_YET: Duration = Duration::from_millis(900);

/// The end-to-end deadline for a dispatch that should settle.
const SETTLES: Duration = Duration::from_secs(30);

/// Flatten one emulated terminal screen into searchable text.
fn screen_text(sessions: &PtyManager, id: &str) -> String {
    sessions
        .screen_rows(id)
        .map(|snapshot| {
            snapshot
                .cells
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|cell| cell.text.as_str())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Spin until `check` passes, or fail.
async fn wait_for(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for: {what}");
}

/// Open a session the way an operator does: theirs from birth, and running
/// whatever `script` says.
fn operator_session(
    sessions: &PtyManager,
    cwd: &str,
    env: &HashMap<String, String>,
    script: &str,
) -> String {
    sessions
        .open(LaunchSpec {
            provider: HarnessProvider::Codex,
            bin: "/bin/sh".to_string(),
            cwd: cwd.to_string(),
            env: env.clone(),
            extra_args: vec!["-c".to_string(), script.to_string()],
            skip_permissions: false,
            label: "you:codex".to_string(),
            model: None,
            session_id: None,
            control: SessionControl::User,
            origin: SessionOrigin::User,
            name: None,
            mcp_grant_session: None,
        })
        .expect("the operator's session must start")
}

/// A harness that answers the *hand-back* turn and nothing before it.
///
/// Modelled on the real sequence: the first prompt starts a turn that never
/// finishes (the operator interrupts it by taking the session), everything the
/// person then types is read and echoed but settles nothing, and the turn is
/// only completed by the prompt the executor injects on hand-back — which is
/// matched by its own wording, so a test cannot pass by accident on the
/// operator's typing.
fn handback_harness_script(rollout: &str, cwd: &str, reply: &str) -> String {
    format!(
        r#"
printf 'ready\r\n'
read -r first
printf 'started: %s\r\n' "$first"
printf '{{"type":"session_meta","payload":{{"session_id":"sess-handback","cwd":"{cwd}"}}}}\n' >> '{rollout}'
printf '{{"type":"event_msg","payload":{{"type":"task_started","turn_id":"t1"}}}}\n' >> '{rollout}'
printf '{{"type":"event_msg","payload":{{"type":"agent_message","message":"halfway through the migration","phase":"main"}}}}\n' >> '{rollout}'
while read -r line; do
  printf 'read: %s\r\n' "$line"
  case "$line" in
    *"you now have it back"*)
      printf '{{"type":"event_msg","payload":{{"type":"task_started","turn_id":"t2"}}}}\n' >> '{rollout}'
      printf '{{"type":"event_msg","payload":{{"type":"task_complete","turn_id":"t2","last_agent_message":"{reply}"}}}}\n' >> '{rollout}'
      ;;
  esac
done
"#
    )
}

#[tokio::test]
async fn a_held_session_is_skipped_and_a_sibling_orchestrator_session_serves_the_task() {
    // Candidacy, at session grain (spec §4.1). The conversation already has a
    // session of its own, idle and the orchestrator's. The operator then opens a
    // session of their own in the same checkout and keeps it. The next dispatch
    // must go to the sibling and leave the person alone — where before, the hold
    // on *their* session refused the whole workspace, and this task failed with
    // an idle harness of its own sitting right beside it.
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let rollout = dir.path().join("rollout-sibling.jsonl");
    let script = conversational_harness_script(&rollout.to_string_lossy(), &cwd);
    let (executor, env) = harness(dir.path(), &cwd);
    let sessions = executor.sessions_for_test();

    // The sibling, made the way one really appears: a turn ran in it and
    // released it.
    let first = tokio::time::timeout(
        SETTLES,
        executor
            .clone()
            .run_for_test(options(&env, "peer-bob", &script, &cwd)),
    )
    .await
    .expect("the first turn settles")
    .expect("the first turn succeeds");
    assert_eq!(first.reply, "answer 1");
    let sibling = sessions
        .rows()
        .into_iter()
        .find(|row| row.state.is_running())
        .expect("the conversation's session stays open")
        .id;

    // Now a person opens their own session in the same checkout, and keeps it.
    let held = operator_session(&sessions, &cwd, &env, "sleep 30");
    wait_for("the operator's session running", || {
        sessions.row(&held).is_some_and(|r| r.state.is_running())
    })
    .await;

    let second = tokio::time::timeout(
        SETTLES,
        executor
            .clone()
            .run_for_test(options(&env, "peer-bob", &script, &cwd)),
    )
    .await
    .expect("the dispatch must settle")
    .expect("a person in one session must not fail a dispatch");

    assert_eq!(
        second.reply, "answer 2",
        "the task must be served by the orchestrator's own session"
    );
    assert_eq!(
        sessions.rows().len(),
        2,
        "reuse, not a third harness: {:?}",
        sessions
            .rows()
            .iter()
            .map(|r| r.label.clone())
            .collect::<Vec<_>>()
    );
    let held_row = sessions
        .row(&held)
        .expect("the operator keeps their session");
    assert_eq!(
        held_row.control,
        SessionControl::User,
        "a dispatch must never take a session out from under a person"
    );
    assert!(
        !held_row.busy,
        "a held session must not even be claimed, let alone written into"
    );
    assert!(
        sessions
            .row(&sibling)
            .is_some_and(|row| row.state.is_running()),
        "the session that served the task is the one that was already there"
    );

    sessions.shutdown();
}

#[tokio::test]
async fn with_only_a_held_session_the_work_queues_and_runs_on_hand_back() {
    // Serialization, at strategy grain (spec §2.3). Nothing to reuse and a
    // person writing in the checkout: under `strategy: checkout` the tree takes
    // one writer at a time, so the dispatch cannot start a rival harness beside
    // them — and no longer fails instead. It waits, and the moment the session
    // comes back it runs.
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let (executor, env) = harness(dir.path(), &cwd);
    let sessions = executor.sessions_for_test();

    let held = operator_session(&sessions, &cwd, &env, "sleep 30");
    wait_for("the operator's session running", || {
        sessions.row(&held).is_some_and(|r| r.state.is_running())
    })
    .await;

    let rollout = dir.path().join("rollout-queued.jsonl");
    let script = fake_harness_script(
        &rollout.to_string_lossy(),
        &cwd,
        "ran once the tree was free",
    );
    let before = sessions.rows().len();
    let mut run = tokio::spawn({
        let (executor, env, script, cwd) = (executor.clone(), env.clone(), script, cwd.clone());
        // Bounded — an agent-targeted dispatch, which never reuses and therefore
        // always needs a session of its own.
        async move {
            executor
                .run_for_test(options(&env, "", &script, &cwd))
                .await
        }
    });

    let waiting = tokio::time::timeout(NOT_YET, &mut run).await;
    assert!(
        waiting.is_err(),
        "the dispatch must queue rather than settle: {waiting:?}"
    );
    assert_eq!(
        sessions.rows().len(),
        before,
        "queuing must not open a second writer in the operator's checkout"
    );

    // The operator hands their session back; the queue drains.
    assert!(sessions.set_control(&held, SessionControl::Orchestrator));
    let result = tokio::time::timeout(SETTLES, run)
        .await
        .expect("the queued dispatch must settle once the tree is free")
        .expect("no panic")
        .expect("a queued dispatch must run, not fail");
    assert!(
        result.reply.contains("ran once the tree was free"),
        "got: {result:?}"
    );

    sessions.shutdown();
}

#[tokio::test]
async fn a_queue_that_outlives_its_budget_fails_loudly_rather_than_silently() {
    // The other half of the queue, and the reason the shared held prefix still
    // exists. Control state is not advertised any more, so an orchestrator
    // cannot see that a task is parked behind a person — which makes waiting
    // forever indistinguishable from losing the task. The wait is therefore
    // bounded by the caller's own idle ceiling and ends in a real, retryable
    // error the hub settles as `RunError::Held`.
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let (executor, env) = harness(dir.path(), &cwd);
    let sessions = executor.sessions_for_test();

    let held = operator_session(&sessions, &cwd, &env, "sleep 30");
    wait_for("the operator's session running", || {
        sessions.row(&held).is_some_and(|r| r.state.is_running())
    })
    .await;

    let script = fake_harness_script(
        &dir.path().join("rollout-never.jsonl").to_string_lossy(),
        &cwd,
        "never runs",
    );
    let mut opts = options(&env, "", &script, &cwd);
    opts.timeout_ms = 400;

    let error = tokio::time::timeout(SETTLES, executor.clone().run_for_test(opts))
        .await
        .expect("the dispatch must settle")
        .expect_err("a queue that never drains must end in an error");

    assert!(
        error.starts_with(medulla::daemon::HARNESS_HELD_PREFIX),
        "the refusal must carry the shared prefix so the hub settles it as Held \
         — retryable, not a task the harness attempted and failed: {error}"
    );
    assert_eq!(
        sessions.rows().len(),
        1,
        "giving up must not leave a harness in the operator's tree"
    );

    sessions.shutdown();
}

#[tokio::test]
async fn a_mid_turn_takeover_suspends_the_turn_and_hands_back_its_result() {
    // The centre of the phase, and one test because it is one story: the
    // operator takes a running session, the turn suspends instead of being
    // discarded, they work in it, they hand it back, and the *same* dispatch
    // answers — from the same session, under the same task id, because it never
    // stopped being the same call.
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let rollout = dir.path().join("rollout-suspend.jsonl");
    let script = handback_harness_script(
        &rollout.to_string_lossy(),
        &cwd,
        "finished after the operator handed it back",
    );

    let (executor, env) = harness(dir.path(), &cwd);
    let sessions = executor.sessions_for_test();
    let (session_tx, session_rx) = tokio::sync::oneshot::channel();
    // The status details the peer is shown. The control markers among them are
    // what pause and resume the requester's no-progress watchdog, so the worker
    // emitting them is half of a contract whose other half lives in
    // `hub::tests::held_watchdog` — and two halves that only ever see their own
    // side of a text prefix are two halves that drift.
    let reported: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let mut opts = options(&env, "peer-bob", &script, &cwd);
    opts.on_event = Some({
        let reported = reported.clone();
        Box::new(
            move |event: &medulla::daemon::mappers::HarnessSemanticEvent| {
                if let Some(detail) = medulla::daemon::status_detail(&event.event) {
                    reported.lock().unwrap().push(detail);
                }
            },
        )
    });
    opts.on_session = Some(Box::new(move |id| {
        let _ = session_tx.send(id);
    }));
    let mut run = tokio::spawn(executor.clone().run_for_test(opts));

    let id = tokio::time::timeout(Duration::from_secs(10), session_rx)
        .await
        .expect("the executor reports its session")
        .expect("the report channel stays open");
    wait_for("the delegated turn to be under way", || {
        screen_text(&sessions, &id).contains("halfway through the migration")
            || std::fs::read_to_string(&rollout)
                .is_ok_and(|text| text.contains("halfway through the migration"))
    })
    .await;

    // The operator takes it, mid-turn.
    assert!(sessions.set_control(&id, SessionControl::User));

    let suspended = tokio::time::timeout(NOT_YET, &mut run).await;
    assert!(
        suspended.is_err(),
        "a takeover must suspend the turn, not end it: {suspended:?}"
    );
    let row = sessions
        .row(&id)
        .expect("the session survives the takeover");
    assert!(
        row.state.is_running(),
        "the operator's session must not be closed under them"
    );
    assert_eq!(row.control, SessionControl::User);
    assert!(
        row.busy,
        "the turn is suspended, not abandoned — the session still owes an answer"
    );

    // …and works in it. Their input is read by the same harness and settles
    // nothing: only the hand-back prompt completes the turn.
    sessions
        .write(&id, b"i fixed the migration myself\r")
        .expect("the operator can type into the session they hold");
    wait_for("the operator's input to reach the harness", || {
        screen_text(&sessions, &id).contains("read: i fixed the migration myself")
    })
    .await;
    assert!(
        !run.is_finished(),
        "the operator working in the session must not settle the task"
    );

    // Hand-back: a fresh turn in that same session produces the task's result.
    assert!(sessions.set_control(&id, SessionControl::Orchestrator));
    let result = tokio::time::timeout(SETTLES, run)
        .await
        .expect("the hand-back turn must settle the task")
        .expect("no panic")
        .expect("a hand-back must produce a result, not an error");

    assert!(
        result
            .reply
            .contains("finished after the operator handed it back"),
        "the result must come from the hand-back turn: {result:?}"
    );
    assert_eq!(
        result.session_id.as_deref(),
        Some("sess-handback"),
        "the hand-back turn runs in the session that saw both the agent's work \
         and the operator's — it is the only context that saw either"
    );
    assert_eq!(
        sessions.rows().len(),
        1,
        "no second session was opened for the hand-back"
    );
    assert!(
        result.events > 0,
        "the suspended turn's fold is retained across the hold, not restarted"
    );
    let reported = reported.lock().unwrap().clone();
    assert!(
        reported
            .iter()
            .any(|detail| detail.starts_with(medulla::daemon::SESSION_HELD_STATUS_PREFIX)),
        "the hold must be announced to the requester — it is what pauses the \
         no-progress watchdog: {reported:?}"
    );
    assert!(
        reported
            .iter()
            .any(|detail| detail.starts_with(medulla::daemon::SESSION_RESUMED_STATUS_PREFIX)),
        "and the hand-back must end it, or the watchdog never resumes: {reported:?}"
    );

    sessions.shutdown();
}

#[tokio::test]
async fn the_idle_watchdog_does_not_fire_across_a_hold() {
    // §5: "watchdog paused while control = user; resumes on hand-back". The
    // caller's ceiling here is a fraction of the hold, so an unpaused clock
    // would kill the task — and would kill it *while a person was working in
    // the session*, which is the worst possible moment to close a pty.
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_string_lossy().into_owned();
    let rollout = dir.path().join("rollout-watchdog.jsonl");
    let script = handback_harness_script(&rollout.to_string_lossy(), &cwd, "survived the hold");

    let (executor, env) = harness(dir.path(), &cwd);
    let sessions = executor.sessions_for_test();
    let (session_tx, session_rx) = tokio::sync::oneshot::channel();
    let mut opts = options(&env, "peer-bob", &script, &cwd);
    // A ceiling far shorter than the hold below. `[host].taskTimeoutMs` is ten
    // minutes in the field; the ratio is what is under test, not the number.
    opts.timeout_ms = 500;
    opts.on_session = Some(Box::new(move |id| {
        let _ = session_tx.send(id);
    }));
    let run = tokio::spawn(executor.clone().run_for_test(opts));

    let id = tokio::time::timeout(Duration::from_secs(10), session_rx)
        .await
        .expect("the executor reports its session")
        .expect("the report channel stays open");
    wait_for("the delegated turn to be under way", || {
        std::fs::read_to_string(&rollout).is_ok_and(|text| text.contains("task_started"))
    })
    .await;
    assert!(sessions.set_control(&id, SessionControl::User));

    // Six times the ceiling, held.
    tokio::time::sleep(Duration::from_millis(3_000)).await;
    assert!(
        !run.is_finished(),
        "a held task must neither time out nor settle"
    );
    assert!(
        sessions.row(&id).is_some_and(|row| row.state.is_running()),
        "an idle-timeout would have stopped the harness the operator is using"
    );

    assert!(sessions.set_control(&id, SessionControl::Orchestrator));
    let result = tokio::time::timeout(SETTLES, run)
        .await
        .expect("the hand-back turn must settle")
        .expect("no panic")
        .expect("held time must not count against the idle ceiling");
    assert!(
        result.reply.contains("survived the hold"),
        "got: {result:?}"
    );

    sessions.shutdown();
}
