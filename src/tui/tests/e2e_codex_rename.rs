//! End-to-end coverage for the Codex thread-rename poller.
//!
//! Codex persists a `/rename` in `session_index.jsonl` instead of the terminal
//! stream, and the transcript executor reads that file exactly once — on the
//! poll where the session is first located. A session that is then held, idle,
//! or retained never runs another turn, so without a second reader the rail
//! would sit on the old name until a new delegated turn happened to start.
//!
//! This drives the real PTY path and renames the session *after* its turn has
//! started: the executor's one-shot read is guaranteed to have already missed
//! the new name, so only the per-session poller can put it on the row.

#![cfg(unix)]

use std::collections::HashMap;
use std::fs;
use std::time::{Duration, Instant};

use medulla::daemon::providers::{Abort, RunTaskOptions};
use medulla::protocol::HarnessProvider;
use medulla::sessions::SessionClass;
use medulla_tui::worker::executor::PtySessionExecutor;
use medulla_tui::worker::pty::PtyManager;

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

#[tokio::test(flavor = "multi_thread")]
async fn a_rename_after_the_turn_started_reaches_the_row_without_a_new_turn() {
    let temp = tempfile::tempdir().expect("a temporary workspace");
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let sessions_dir = temp.path().join("codex").join("sessions");
    fs::create_dir_all(&sessions_dir).unwrap();
    let cwd = workspace.to_string_lossy().into_owned();
    let sessions_str = sessions_dir.to_string_lossy().into_owned();
    // `rollout-*.jsonl` — codex transcript discovery matches on that prefix.
    let rollout = sessions_dir.join("rollout-rename.jsonl");
    let rollout = rollout.to_string_lossy().into_owned();

    let script = format!(
        r#"
printf 'codex ready\r\n'
read -r prompt
printf 'task started: %s\r\n' "$prompt"
printf '{{"type":"session_meta","payload":{{"session_id":"codex-rename-e2e","cwd":"{cwd}"}}}}\n' >> '{rollout}'
printf '{{"type":"event_msg","payload":{{"type":"task_started","turn_id":"turn-1"}}}}\n' >> '{rollout}'
while read -r line; do
  printf 'peered: %s\r\n' "$line"
done
"#
    );

    // A stale index is planted before the session runs: the executor's one-shot
    // label read can therefore never find the rename, whenever it happens.
    // Only the poller that re-reads this file on a timer can.
    let index = temp.path().join("codex").join("session_index.jsonl");
    fs::write(
        &index,
        r#"{"id":"some-other-session","thread_name":"untouched"}"#,
    )
    .unwrap();

    let mut env = HashMap::new();
    env.insert(
        "PATH".to_string(),
        std::env::var("PATH").unwrap_or_default(),
    );
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    env.insert("MEDULLA_CODEX_SESSIONS_DIR".to_string(), sessions_str);
    env.insert("MEDULLA_CODEX_BIN".to_string(), "/bin/sh".to_string());

    let sessions = PtyManager::new();
    let run_task =
        PtySessionExecutor::new(sessions.clone(), env.clone(), cwd.clone()).into_run_task();
    let (session_tx, session_rx) = tokio::sync::oneshot::channel();
    let run = tokio::spawn((run_task)(RunTaskOptions {
        hooks: medulla::harness_hooks::HooksConfig::default(),
        transport: Default::default(),
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

    // The turn has started: this id is only recorded on the fold that locates
    // the session, by which point the executor's one-shot label read has run.
    wait_for("the Codex session to be located", || {
        sessions
            .row(&id)
            .is_some_and(|row| row.session_id.as_deref() == Some("codex-rename-e2e"))
    })
    .await;

    // Rename it *now*, after the executor's only read has passed. The poller is
    // the sole remaining reader of the index.
    fs::write(
        &index,
        r#"{"id":"codex-rename-e2e","thread_name":"Ship the sidebar"}"#,
    )
    .unwrap();
    wait_for("the per-session poller to reach the renamed thread", || {
        sessions
            .row(&id)
            .is_some_and(|row| row.thread_name.as_deref() == Some("ship-sidebar"))
    })
    .await;
    assert!(
        !run.is_finished(),
        "the rename was picked up by the poller, not by a second turn"
    );

    sessions.close(&id);
}
