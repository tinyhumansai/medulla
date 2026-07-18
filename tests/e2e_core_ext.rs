//! Additional end-to-end tests for the core-js runtime path, complementing
//! `e2e_core.rs`. Uses the configurable [`mock_core`] stub to reach the branches
//! the base scenarios skip: existing-thread adoption, snapshot seeding, the full
//! steering / fleet RPC matrix, error surfacing, the `resync.required` snapshot
//! carry, the stall / connection-drop transitions, and malformed / oversize frames.

mod support;

#[path = "support/mock_core.rs"]
mod mock_core;

use std::path::Path;
use std::time::Duration;

use serde_json::json;

use medulla::runtime::core::CoreRuntime;
use medulla::runtime::core_client::{CallError, CoreClient};
use medulla::runtime::{Runtime, StreamState, WorkerOp};
use medulla::ui::agents::derive_agent_lanes;

use mock_core::{MockCore, MockCoreConfig};
use support::wait_until;

const T: Duration = Duration::from_secs(5);

async fn connect(sock: &Path) -> CoreRuntime {
    let (client, rx) = CoreClient::connect(sock).await.unwrap();
    CoreRuntime::connect(client, rx, "test").await.unwrap()
}

fn tmp_sock() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("core.sock");
    (dir, sock)
}

// connect() adopts the first thread `thread.list` reports (no create) and seeds its
// snapshot chat + tasks.
#[tokio::test]
async fn adopts_existing_thread_and_seeds_snapshot() {
    let (_dir, sock) = tmp_sock();
    let cfg = MockCoreConfig {
        existing_thread: Some("th_test".into()),
        subscribe_snapshot: Some(json!({
            "at": 1000,
            "chat": [
                {"role": "user", "body": "old q"},
                {"role": "assistant", "body": "old a"}
            ],
            "tasks": [{
                "taskId": "t1", "cycleId": "cyc:app:th_test:1",
                "status": "done", "instruction": "seeded", "digest": "d"
            }],
        })),
        ..Default::default()
    };
    let mock = MockCore::start_with(&sock, cfg).await;
    let rt = connect(&sock).await;

    let snap = rt.snapshot();
    assert_eq!(snap.messages.len(), 2);
    assert_eq!(snap.messages[0].content, "old q");
    assert_eq!(snap.messages[1].content, "old a");
    let lanes = derive_agent_lanes(&snap.events, "CORE", &[]);
    assert!(lanes.iter().any(|l| l.key.contains("/t:t1")));

    // Adopted, not created.
    let calls = mock.calls();
    assert!(calls.contains(&"thread.subscribe".to_string()));
    assert!(!calls.contains(&"thread.create".to_string()), "{calls:?}");
}

// user / assistant events fold into the message list.
#[tokio::test]
async fn user_assistant_events_fold_to_messages() {
    let (_dir, sock) = tmp_sock();
    let mock = MockCore::start(&sock).await;
    let rt = connect(&sock).await;

    let cyc = "cyc:app:th_test:1";
    mock.push_event(1, cyc, json!({"kind": "user", "body": "hey"}));
    mock.push_event(2, cyc, json!({"kind": "assistant", "body": "hello"}));

    wait_until("both turns fold", T, || rt.snapshot().messages.len() == 2).await;
    let snap = rt.snapshot();
    assert_eq!(snap.messages[0].role, "user");
    assert_eq!(snap.messages[1].content, "hello");
}

// A failing cycle.submit clears the optimistic running flag and surfaces the error.
#[tokio::test]
async fn submit_error_clears_running() {
    let (_dir, sock) = tmp_sock();
    let cfg = MockCoreConfig::default().with_error("cycle.submit", "cycle.rejected");
    let _mock = MockCore::start_with(&sock, cfg).await;
    let rt = connect(&sock).await;

    let err = rt.submit("go".into()).await;
    assert!(err.is_err(), "submit should surface the rejection");
    wait_until("running clears after submit error", T, || {
        !rt.snapshot().running
    })
    .await;
}

// abort routes to cycle.abort using the cycleId from the accepted submit.
#[tokio::test]
async fn abort_issues_cycle_abort() {
    let (_dir, sock) = tmp_sock();
    let mock = MockCore::start(&sock).await;
    let rt = connect(&sock).await;

    rt.submit("work".into()).await.unwrap();
    rt.abort();
    wait_until("cycle.abort issued", T, || {
        mock.calls().iter().any(|m| m == "cycle.abort")
    })
    .await;
    let params = mock.params_of("cycle.abort").unwrap();
    assert_eq!(params["cycleId"], json!("cyc:app:th_test:1"));
}

// Steering ops issue their RPCs.
#[tokio::test]
async fn steering_ops_issue_rpcs() {
    let (_dir, sock) = tmp_sock();
    let mock = MockCore::start(&sock).await;
    let rt = connect(&sock).await;

    rt.answer_question("cyc1".into(), "q1".into(), "yes".into());
    rt.cancel_task("cyc1".into(), "t1".into());

    wait_until("both steering rpcs issued", T, || {
        let c = mock.calls();
        c.iter().any(|m| m == "question.answer") && c.iter().any(|m| m == "task.cancel")
    })
    .await;
    assert_eq!(
        mock.params_of("question.answer").unwrap()["body"],
        json!("yes")
    );
    assert_eq!(
        mock.params_of("task.cancel").unwrap()["taskId"],
        json!("t1")
    );
}

// new_session creates a fresh core thread and re-subscribes.
#[tokio::test]
async fn new_session_creates_and_subscribes() {
    let (_dir, sock) = tmp_sock();
    let mock = MockCore::start(&sock).await;
    let rt = connect(&sock).await;

    rt.new_session();
    wait_until("a second thread.create is issued", T, || {
        mock.calls()
            .iter()
            .filter(|m| *m == "thread.create")
            .count()
            >= 2
    })
    .await;
    // The freshly-created thread is bound and reset.
    let snap = rt.snapshot();
    assert!(snap.messages.is_empty());
    assert!(!snap.session_id.is_empty());
}

// fork forks the core thread and adds a local child thread.
#[tokio::test]
async fn fork_forks_core_thread() {
    let (_dir, sock) = tmp_sock();
    let mock = MockCore::start(&sock).await;
    let rt = connect(&sock).await;

    let child = rt.fork(Some("branch".into()));
    assert_eq!(rt.snapshot().active_thread_id, child);
    assert_eq!(rt.snapshot().threads.len(), 2);
    wait_until("thread.fork issued", T, || {
        mock.calls().iter().any(|m| m == "thread.fork")
    })
    .await;
    assert_eq!(
        mock.params_of("thread.fork").unwrap()["threadId"],
        json!("th_test")
    );
}

// resume_chat resumes + subscribes and rebuilds the transcript from the snapshot.
#[tokio::test]
async fn resume_rebuilds_from_snapshot() {
    let (_dir, sock) = tmp_sock();
    let cfg = MockCoreConfig {
        subscribe_snapshot: Some(json!({
            "at": 1,
            "chat": [
                {"role": "user", "body": "resumed q"},
                {"role": "assistant", "body": "resumed a"}
            ],
            "tasks": [],
        })),
        ..Default::default()
    };
    let mock = MockCore::start_with(&sock, cfg).await;
    let rt = connect(&sock).await;

    rt.resume_chat("th_other".into()).await.unwrap();
    let snap = rt.snapshot();
    assert_eq!(snap.session_id, "th_other");
    assert_eq!(snap.messages.len(), 2);
    assert_eq!(snap.messages[0].content, "resumed q");
    let calls = mock.calls();
    assert!(calls.contains(&"thread.resume".to_string()));
}

// inspect_context maps `context.inspect` chunks into ContextItems.
#[tokio::test]
async fn inspect_context_maps_chunks() {
    let (_dir, sock) = tmp_sock();
    let _mock = MockCore::start(&sock).await;
    let rt = connect(&sock).await;

    // Establish a latest cycle id so inspect_context has something to query.
    rt.submit("go".into()).await.unwrap();

    let items = rt.inspect_context().await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].content, "remembered fact");
    assert_eq!(items[0].kind, "note");
    assert_eq!(items[0].ref_, "ctx-1");
}

// worker.update flows through worker_op and re-pulls the authoritative list.
#[tokio::test]
async fn worker_update_round_trips() {
    let (_dir, sock) = tmp_sock();
    let mock = MockCore::start(&sock).await;
    let rt = connect(&sock).await;

    rt.worker_op(WorkerOp::Add {
        address: Some("addr".into()),
        handle: None,
        label: Some("old".into()),
        harness: None,
    })
    .await
    .unwrap();
    let id = rt.workers()[0].id.clone();

    let mut patch = serde_json::Map::new();
    patch.insert("label".into(), json!("renamed"));
    rt.worker_op(WorkerOp::Update {
        id: id.clone(),
        patch,
    })
    .await
    .unwrap();

    assert!(mock.calls().iter().any(|m| m == "worker.update"));
    assert_eq!(rt.workers()[0].label.as_deref(), Some("renamed"));
}

// A dropped connection ends the fold loop and the stream reads as stalled.
#[tokio::test]
async fn connection_drop_marks_stalled() {
    let (_dir, sock) = tmp_sock();
    let mock = MockCore::start(&sock).await;
    let rt = connect(&sock).await;

    mock.close();
    wait_until("stream stalls after drop", T, || {
        matches!(rt.stream_state(), Some(StreamState::Stalled))
    })
    .await;
}

// The stall watchdog reports Stalled once a running cycle goes silent past the
// (shortened) threshold, with no connection drop.
#[tokio::test]
async fn stall_watchdog_marks_stalled() {
    let (_dir, sock) = tmp_sock();
    let _mock = MockCore::start(&sock).await;
    let rt = connect(&sock).await;
    rt.set_stall_ms(50);

    rt.submit("go".into()).await.unwrap();
    assert!(rt.snapshot().running);
    wait_until("silence trips the stall guard", T, || {
        matches!(rt.stream_state(), Some(StreamState::Stalled))
    })
    .await;
}

// A `snapshot.get` that answers with a `resync.required` error still hands back the
// durable snapshot in `data`, which the resync path folds into a lane.
#[tokio::test]
async fn resync_required_error_carries_snapshot() {
    let (_dir, sock) = tmp_sock();
    let cfg = MockCoreConfig::default().with_error_data(
        "snapshot.get",
        "resync.required",
        json!({
            "snapshot": {
                "at": 1,
                "chat": [],
                "tasks": [{
                    "taskId": "t_err", "cycleId": "cyc:app:th_test:1",
                    "status": "running", "instruction": "from-error", "depth": 1
                }],
            }
        }),
    );
    let mock = MockCore::start_with(&sock, cfg).await;
    let rt = connect(&sock).await;

    let cyc = "cyc:app:th_test:1";
    mock.push_event(1, cyc, json!({"kind": "cycle_start", "cycleId": cyc}));
    // Jump the seq to force a gap → snapshot.get → resync.required (with data).
    mock.push_event(
        10,
        cyc,
        json!({"kind": "task_event", "taskId": "t1", "eventKind": "text", "content": "late"}),
    );

    wait_until("snapshot.get invoked on the gap", T, || {
        mock.calls().iter().any(|m| m == "snapshot.get")
    })
    .await;
    wait_until("resync rebuilt the lane from the error snapshot", T, || {
        derive_agent_lanes(&rt.snapshot().events, "CORE", &[])
            .iter()
            .any(|l| l.key.contains("/t:t_err"))
    })
    .await;
}

// A malformed (non-JSON) frame is skipped, not fatal: a following valid event folds.
#[tokio::test]
async fn malformed_frame_is_skipped() {
    let (_dir, sock) = tmp_sock();
    let mock = MockCore::start(&sock).await;
    let rt = connect(&sock).await;

    mock.push_raw_line("this is not json at all");
    mock.push_event(
        1,
        "cyc:app:th_test:1",
        json!({"kind": "assistant", "body": "survived"}),
    );

    wait_until("valid event still folds", T, || {
        rt.snapshot()
            .messages
            .iter()
            .any(|m| m.content == "survived")
    })
    .await;
}

// An over-1-MiB inbound frame is a protocol error: the read loop stops and the
// stream reads as stalled.
#[tokio::test]
async fn oversize_frame_terminates_stream() {
    let (_dir, sock) = tmp_sock();
    let mock = MockCore::start(&sock).await;
    let rt = connect(&sock).await;

    mock.push_oversize_frame();
    wait_until("oversize frame terminates the stream", T, || {
        matches!(rt.stream_state(), Some(StreamState::Stalled))
    })
    .await;
}

// An `initialize` error fails the handshake and thus the whole connect.
#[tokio::test]
async fn initialize_error_fails_connect() {
    let (_dir, sock) = tmp_sock();
    let cfg = MockCoreConfig::default().with_error("initialize", "protocol.mismatch");
    let _mock = MockCore::start_with(&sock, cfg).await;

    let (client, rx) = CoreClient::connect(&sock).await.unwrap();
    let result = CoreRuntime::connect(client, rx, "test").await;
    assert!(result.is_err(), "handshake failure must fail connect");
}

// ---------------------------------------------------------------------------
// CoreClient transport-level branches
// ---------------------------------------------------------------------------

// A response missing the promised field surfaces as a transport error.
#[tokio::test]
async fn missing_cycle_id_is_transport_error() {
    let (_dir, sock) = tmp_sock();
    let mut cfg = MockCoreConfig::default();
    cfg.responses.insert("cycle.submit".into(), json!({}));
    let _mock = MockCore::start_with(&sock, cfg).await;
    let (client, _rx) = CoreClient::connect(&sock).await.unwrap();

    let err = client
        .cycle_submit("th_test", "hi", None)
        .await
        .unwrap_err();
    assert!(matches!(err, CallError::Transport(_)), "{err}");
}

// An outbound frame over the 1 MiB cap is rejected before it hits the socket.
#[tokio::test]
async fn oversize_outbound_frame_rejected() {
    let (_dir, sock) = tmp_sock();
    let _mock = MockCore::start(&sock).await;
    let (client, _rx) = CoreClient::connect(&sock).await.unwrap();

    let huge = "x".repeat(medulla::runtime::core_client::MAX_FRAME_BYTES + 1);
    let err = client
        .request("cycle.submit", json!({ "input": huge }))
        .await
        .unwrap_err();
    match err {
        CallError::Transport(m) => assert!(m.contains("1 MiB"), "{m}"),
        other => panic!("expected transport error, got {other}"),
    }
}

// A connection dropped while a request is in flight fails that request rather than
// hanging it.
#[tokio::test]
async fn request_drop_mid_flight_is_transport_error() {
    let (_dir, sock) = tmp_sock();
    let cfg = MockCoreConfig {
        close_on: Some("cycle.abort".into()),
        ..Default::default()
    };
    let _mock = MockCore::start_with(&sock, cfg).await;
    let (client, _rx) = CoreClient::connect(&sock).await.unwrap();

    // On disconnect the read loop drains every outstanding request with a synthetic
    // `transport.closed` RPC error rather than leaving it hung.
    let err = client.cycle_abort("c1").await.unwrap_err();
    assert!(
        matches!(err, CallError::Transport(_)) || err.rpc_code() == Some("transport.closed"),
        "{err}"
    );
}
