//! End-to-end coverage for taking a live session back from the orchestrator —
//! and giving it back again.
//!
//! A deterministic shell stands in for Codex, but everything around it is the
//! production path: a task opens a real PTY, its prompt is injected, its rollout
//! is tailed, and the operator takes control while the turn is still running.
//! The same PTY must then accept the operator's input instead of being kept or
//! closed by the task executor.
//!
//! What the takeover must *not* do is end the task. It used to: the executor
//! returned `harness held by operator` the moment control flipped, discarding
//! everything the turn had produced and telling the orchestrator its work had
//! failed — for the entirely ordinary event of a person opening the session to
//! look at it. The turn now suspends, the session keeps it, and the hand-back
//! runs a fresh turn *in that same session* whose answer is emitted as the
//! task's own result. That path is the only way a held task ever reaches a
//! result, so it is exercised end to end here rather than only in unit tests.

#![cfg(unix)]

use std::collections::HashMap;
use std::time::{Duration, Instant};

use medulla::daemon::providers::{Abort, RunTaskOptions};
use medulla::protocol::HarnessProvider;
use medulla::sessions::SessionClass;
use medulla_tui::worker::executor::PtySessionExecutor;
use medulla_tui::worker::pty::{PtyManager, SessionControl};

/// Maximum time for a real child process or PTY reader to make progress.
const PATIENCE: Duration = Duration::from_secs(10);

/// Spin until `check` passes or the end-to-end deadline expires.
async fn wait_for(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for: {what}");
}

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

#[tokio::test(flavor = "multi_thread")]
async fn taking_back_a_running_codex_session_yields_the_same_pty_to_the_operator() {
    let temp = tempfile::tempdir().expect("a temporary workspace");
    let cwd = temp.path().to_string_lossy().into_owned();
    // `rollout-*.jsonl` — codex transcript discovery matches on that prefix, and
    // a file named otherwise is invisible to the tailer.
    let rollout = temp.path().join("rollout-takeover.jsonl");
    let rollout = rollout.to_string_lossy().into_owned();
    // Reads for ever, so the operator's typing and the hand-back prompt arrive
    // through the same channel a real session would deliver them on. Only the
    // hand-back prompt — matched by its own wording — completes a turn, so
    // nothing here can settle the task by accident.
    let script = format!(
        r#"
printf 'codex ready\r\n'
read -r prompt
printf 'task started: %s\r\n' "$prompt"
printf '{{"type":"session_meta","payload":{{"session_id":"codex-takeover-e2e","cwd":"{cwd}"}}}}\n' >> '{rollout}'
printf '{{"type":"event_msg","payload":{{"type":"task_started","turn_id":"turn-1"}}}}\n' >> '{rollout}'
while read -r line; do
  printf 'operator typed: %s\r\n' "$line"
  case "$line" in
    *"you now have it back"*)
      printf '{{"type":"event_msg","payload":{{"type":"task_started","turn_id":"turn-2"}}}}\n' >> '{rollout}'
      printf '{{"type":"event_msg","payload":{{"type":"task_complete","turn_id":"turn-2","last_agent_message":"reviewed the session and finished it"}}}}\n' >> '{rollout}'
      ;;
  esac
done
"#
    );

    let mut env = HashMap::new();
    env.insert(
        "PATH".to_string(),
        std::env::var("PATH").unwrap_or_default(),
    );
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    env.insert("MEDULLA_CODEX_SESSIONS_DIR".to_string(), cwd.clone());
    env.insert("MEDULLA_CODEX_BIN".to_string(), "/bin/sh".to_string());

    let sessions = PtyManager::new();
    let run_task =
        PtySessionExecutor::new(sessions.clone(), env.clone(), cwd.clone()).into_run_task();
    let (session_tx, session_rx) = tokio::sync::oneshot::channel();
    let run = tokio::spawn((run_task)(RunTaskOptions {
        hooks: medulla::harness_hooks::HooksConfig::default(),
        provider: HarnessProvider::Codex,
        prompt: "start delegated work".to_string(),
        cwd,
        env,
        timeout_ms: 30_000,
        model: None,
        agent: None,
        extra_args: vec!["-c".to_string(), script],
        skip_permissions: false,
        conversation: "medulla-orchestrator".to_string(),
        session_class: SessionClass::Bounded,
        resume_session_id: None,
        workspace_context: Default::default(),
        abort: Abort::new(),
        router: None,
        attribution: false,
        on_event: None,
        on_stdin: None,
        on_session: Some(Box::new(move |id| {
            let _ = session_tx.send(id);
        })),
        on_workspace_context: None,
    }));

    let id = tokio::time::timeout(PATIENCE, session_rx)
        .await
        .expect("the executor reports its PTY")
        .expect("the session report channel stays open");
    wait_for("Codex to start the delegated turn", || {
        screen_text(&sessions, &id).contains("task started: start delegated work")
    })
    .await;

    assert!(sessions.set_control(&id, SessionControl::User));
    let mut run = run;
    let suspended = tokio::time::timeout(Duration::from_secs(2), &mut run).await;
    assert!(
        suspended.is_err(),
        "the turn suspends when the operator takes control; it is not discarded \
         and reported as a failure: {suspended:?}"
    );

    let row = sessions.row(&id).expect("the taken-over session remains");
    assert!(row.state.is_running(), "the taken-over PTY must stay alive");
    assert_eq!(row.control, SessionControl::User);
    assert!(
        row.busy,
        "the delegated turn is suspended, not abandoned — the session still owes \
         this task an answer"
    );

    sessions
        .write(&id, b"back in\r")
        .expect("the operator can type into the same PTY");
    wait_for(
        "the taken-over Codex process to receive operator input",
        || screen_text(&sessions, &id).contains("operator typed: back in"),
    )
    .await;
    assert!(
        !run.is_finished(),
        "the operator working in the session must not settle the task"
    );

    // Hand it back: the runtime runs a fresh turn in this same session, and its
    // answer is this task's result — the session is the only context that saw
    // both the agent's partial work and the operator's.
    assert!(sessions.set_control(&id, SessionControl::Orchestrator));
    let outcome = tokio::time::timeout(PATIENCE * 3, run)
        .await
        .expect("the hand-back turn settles the task")
        .expect("the executor task does not panic")
        .expect("a hand-back must produce a result, not an error");
    assert!(
        outcome
            .reply
            .contains("reviewed the session and finished it"),
        "the result must come from the hand-back turn: {outcome:?}"
    );
    assert_eq!(
        outcome.session_id.as_deref(),
        Some("codex-takeover-e2e"),
        "the hand-back turn runs in the same session, not a fresh one"
    );

    sessions.close(&id);
}
