//! End-to-end coverage for taking a live session back from the orchestrator.
//!
//! A deterministic shell stands in for Codex, but everything around it is the
//! production path: a task opens a real PTY, its prompt is injected, its rollout
//! is tailed, and the operator takes control while the turn is still running.
//! The same PTY must then accept the operator's input instead of being kept or
//! closed by the task executor.

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
    let rollout = temp.path().join("rollout.jsonl");
    let rollout = rollout.to_string_lossy().into_owned();
    let script = format!(
        r#"
printf 'codex ready\r\n'
read -r prompt
printf 'task started: %s\r\n' "$prompt"
printf '{{"type":"session_meta","payload":{{"session_id":"codex-takeover-e2e","cwd":"{cwd}"}}}}\n' >> '{rollout}'
printf '{{"type":"event_msg","payload":{{"type":"task_started","turn_id":"turn-1"}}}}\n' >> '{rollout}'
read -r operator_input
printf 'operator typed: %s\r\n' "$operator_input"
sleep 30
"#
    );

    let mut env = HashMap::new();
    env.insert(
        "PATH".to_string(),
        std::env::var("PATH").unwrap_or_default(),
    );
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    env.insert("TINYPLACE_CODEX_SESSIONS_DIR".to_string(), cwd.clone());
    env.insert("TINYPLACE_CODEX_BIN".to_string(), "/bin/sh".to_string());

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
    let outcome = tokio::time::timeout(Duration::from_secs(2), run)
        .await
        .expect("the executor yields promptly when the operator takes control")
        .expect("the executor task does not panic");
    let error = outcome.expect_err("the delegated turn is yielded, not reported complete");
    assert!(
        error.starts_with(medulla::daemon::HARNESS_HELD_PREFIX),
        "unexpected takeover error: {error}"
    );

    let row = sessions.row(&id).expect("the taken-over session remains");
    assert!(row.state.is_running(), "the taken-over PTY must stay alive");
    assert_eq!(row.control, SessionControl::User);
    assert!(!row.busy, "the abandoned delegated turn must be released");

    sessions
        .write(&id, b"back in\r")
        .expect("the operator can type into the same PTY");
    wait_for(
        "the taken-over Codex process to receive operator input",
        || screen_text(&sessions, &id).contains("operator typed: back in"),
    )
    .await;

    sessions.close(&id);
}
