//! Full owner → daemon → harness task chain: an owner sends an encrypted task
//! frame, the daemon admits + runs it (on a mock harness CLI, or the real
//! `opencode` binary when present), and the owner receives ack → status →
//! reply frames, all validated end-to-end with ciphertext-only on the wire.

use std::collections::HashMap;
use std::time::Duration;

use medulla::daemon::DaemonRuntime;
use medulla::tinyplace::{HarnessProvider, TaskFrame, TaskFrameKind, TINYPLACE_PROTO};

use crate::helpers::*;
use crate::mock_harness::{MockCli, MockDir, MockProvider};
use crate::mock_signal_server::MockSignalServer;

// Scenario 2: full task chain. An owner sends a `medulla-tinyplace/1` task frame;
// the DAEMON receives it over the mock Signal server, admits + runs it on a MOCK
// harness CLI, and the owner receives ack → status → encrypted reply frames,
// each decrypted and validated end-to-end. Stored server payloads are ciphertext.
#[tokio::test]
async fn task_chain_owner_daemon_mock_harness() {
    let server = MockSignalServer::start().await;
    let owner = make_identity("owner-chain", &server.base_url);
    let worker = make_identity("worker-chain", &server.base_url);
    owner.transport.publish_keys(&owner.signer).await.unwrap();
    worker.transport.publish_keys(&worker.signer).await.unwrap();
    let worker_id = worker.id();

    // The daemon's workspace + a mock claude CLI that emits a rich stream and a
    // final result line the reply is drawn from.
    let work = IdentityDir::new("work-chain");
    let mock = MockCli::new(MockProvider::Claude)
        .thinking("planning the work")
        .tool(
            "read",
            serde_json::json!({ "file_path": "/a/b.rs" }),
            "file contents",
            false,
        )
        .message("intermediate note")
        .claude_result("final answer from worker");
    let mock_dir = MockDir::new();
    let bin = mock_dir.install(&mock);
    let mut env: HashMap<String, String> = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    env.insert("TINYPLACE_CLAUDE_BIN".to_string(), bin);

    let runtime = DaemonRuntime::new(
        daemon_config(
            HarnessProvider::Claude,
            work.path.to_string_lossy().into_owned(),
            env,
        ),
        real_run_task(),
        transport_send(worker.transport.clone()),
    );

    // Owner dispatches the task frame (encrypted, opens the X3DH session).
    owner
        .transport
        .send(
            &worker_id,
            &task_frame(TaskFrameKind::Task, "cyc-1", "do the thing", Some("corr-1")),
        )
        .await
        .unwrap();

    let mut collected: Vec<TaskFrame> = Vec::new();
    let saw_reply = run_chain_until(
        &worker.transport,
        &owner.transport,
        &runtime,
        &mut collected,
        T,
        |frames| frames.iter().any(|f| f.kind == TaskFrameKind::Reply),
    )
    .await;
    assert!(
        saw_reply,
        "owner never received a reply frame: {collected:?}"
    );
    runtime.idle().await;
    // Final drain to collect any trailing frames.
    pump_chain(
        &worker.transport,
        &owner.transport,
        &runtime,
        &mut collected,
    )
    .await;

    // ack("task accepted") first, reply last, statuses in between.
    assert_eq!(collected[0].kind, TaskFrameKind::Ack);
    assert_eq!(collected[0].text, "task accepted");
    let reply = collected
        .iter()
        .find(|f| f.kind == TaskFrameKind::Reply)
        .expect("reply frame");
    assert_eq!(reply.text, "final answer from worker");
    // At least one status frame arrived (thinking / tool activity). Extra stream
    // records are tolerated — assert presence, not an exact count.
    assert!(
        collected.iter().any(|f| f.kind == TaskFrameKind::Status),
        "expected at least one status frame: {collected:?}"
    );
    // Every frame echoes the correlationId and carries the resolved harness.
    for frame in &collected {
        assert_eq!(frame.correlation_id.as_deref(), Some("corr-1"));
        assert_eq!(frame.harness, Some(HarnessProvider::Claude));
        assert_eq!(frame.proto, TINYPLACE_PROTO);
    }

    // Server never saw the task text, the reply, or any status detail in plaintext.
    server.assert_ciphertext_only(&[
        "do the thing",
        "final answer from worker",
        "planning the work",
        "task accepted",
    ]);
}

/// True when a real `opencode` binary is on PATH (drives scenario 3's gate).
fn opencode_available() -> bool {
    std::process::Command::new("opencode")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// Scenario 3: the same owner → daemon chain, but the daemon runs the task on the
// REAL `opencode` binary when it is present on PATH. Gated so CI (no opencode)
// stays green: absent → note + early return. When present, a terminal frame
// (reply or error) must come back — the assertion tolerates a keyless
// environment where opencode may fail rather than answer.
#[tokio::test]
async fn task_chain_real_opencode_when_present() {
    if !opencode_available() {
        eprintln!("skipping: opencode not on PATH");
        return;
    }

    let server = MockSignalServer::start().await;
    let owner = make_identity("owner-oc", &server.base_url);
    let worker = make_identity("worker-oc", &server.base_url);
    owner.transport.publish_keys(&owner.signer).await.unwrap();
    worker.transport.publish_keys(&worker.signer).await.unwrap();
    let worker_id = worker.id();

    let work = IdentityDir::new("work-oc");
    // Inherit the real PATH so run_provider_task discovers opencode; no bin override.
    let mut env: HashMap<String, String> = HashMap::new();
    if let Ok(path) = std::env::var("PATH") {
        env.insert("PATH".to_string(), path);
    }
    let mut config = daemon_config(
        HarnessProvider::Opencode,
        work.path.to_string_lossy().into_owned(),
        env,
    );
    // A tight timeout keeps the test fast even if opencode blocks on a missing key.
    config.task_timeout_ms = 20_000;
    let runtime = DaemonRuntime::new(
        config,
        real_run_task(),
        transport_send(worker.transport.clone()),
    );

    owner
        .transport
        .send(
            &worker_id,
            &task_frame(
                TaskFrameKind::Task,
                "oc-1",
                "print the word ready",
                Some("oc-c1"),
            ),
        )
        .await
        .unwrap();

    let mut collected: Vec<TaskFrame> = Vec::new();
    let saw_terminal = run_chain_until(
        &worker.transport,
        &owner.transport,
        &runtime,
        &mut collected,
        Duration::from_secs(30),
        |frames| {
            frames
                .iter()
                .any(|f| matches!(f.kind, TaskFrameKind::Reply | TaskFrameKind::Error))
        },
    )
    .await;
    runtime.shutdown();
    assert!(
        saw_terminal,
        "real opencode chain produced no terminal frame: {collected:?}"
    );
    // The daemon acked the task before running it, regardless of the outcome.
    assert!(
        collected
            .iter()
            .any(|f| f.kind == TaskFrameKind::Ack && f.text == "task accepted"),
        "expected a task-accepted ack: {collected:?}"
    );
    // Whatever opencode produced stayed encrypted on the wire.
    server.assert_ciphertext_only(&["print the word ready", "task accepted"]);
}
