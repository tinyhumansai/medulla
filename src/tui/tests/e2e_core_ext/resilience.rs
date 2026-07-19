//! Core-runtime resilience and guard branches: malformed / oversize inbound
//! frames, a failed handshake, subscribe pings, submit/resume rejection while
//! running, local thread switching, main-chat listing, empty context, shutdown,
//! worker RPC errors, untracked-thread events, and seq-gap chat rebuilds.

use crate::helpers::*;
use crate::mock_core::{MockCore, MockCoreConfig};
use crate::support::wait_until;

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
    let result = CoreRuntime::connect(client, rx, "test", None).await;
    assert!(result.is_err(), "handshake failure must fail connect");
}

// subscribe() hands out a receiver that pings on the next mutation.
#[tokio::test]
async fn subscribe_pings_on_mutation() {
    let (_dir, sock) = tmp_sock();
    let _mock = MockCore::start(&sock).await;
    let rt = connect(&sock).await;

    let mut rx = rt.subscribe();
    rt.set_async_mode(true);
    assert!(rx.try_recv().is_ok());
    assert!(rt.snapshot().async_mode);
}

// Submitting while a cycle is already running is rejected (optimistic running flag).
#[tokio::test]
async fn submit_rejects_while_running() {
    let (_dir, sock) = tmp_sock();
    let _mock = MockCore::start(&sock).await;
    let rt = connect(&sock).await;

    rt.submit("first".into()).await.unwrap();
    assert!(rt.snapshot().running, "optimistic running flag stays set");
    let err = rt.submit("second".into()).await.unwrap_err();
    assert!(err.to_string().contains("already running"), "{err}");
}

// abort before any cycle has been accepted is a silent no-op (no cycle.abort RPC).
#[tokio::test]
async fn abort_without_cycle_is_noop() {
    let (_dir, sock) = tmp_sock();
    let mock = MockCore::start(&sock).await;
    let rt = connect(&sock).await;

    rt.abort();
    // Give any (erroneously) spawned task a moment to run.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !mock.calls().iter().any(|m| m == "cycle.abort"),
        "no cycle to abort → no RPC"
    );
}

// set_active_thread switches to a known local thread and ignores unknown ids.
#[tokio::test]
async fn set_active_thread_switches_and_ignores_unknown() {
    let (_dir, sock) = tmp_sock();
    let _mock = MockCore::start(&sock).await;
    let rt = connect(&sock).await;

    let child = rt.fork(Some("branch".into()));
    assert_eq!(rt.snapshot().active_thread_id, child);
    rt.set_active_thread("nope".into());
    assert_eq!(
        rt.snapshot().active_thread_id,
        child,
        "unknown id is a no-op"
    );
    rt.set_active_thread("t1".into());
    assert_eq!(rt.snapshot().active_thread_id, "t1");
}

// list_main_chats maps thread.list rows into MainChatSummary entries.
#[tokio::test]
async fn list_main_chats_maps_threads() {
    let (_dir, sock) = tmp_sock();
    let cfg = MockCoreConfig {
        existing_thread: Some("th_test".into()),
        ..Default::default()
    };
    let _mock = MockCore::start_with(&sock, cfg).await;
    let rt = connect(&sock).await;

    let chats = rt.list_main_chats().await.unwrap();
    assert_eq!(chats.len(), 1);
    assert_eq!(chats[0].session_id, "th_test");
    assert_eq!(chats[0].name, "main");
    assert_eq!(chats[0].turns, 2);
}

// resume_chat is refused while a thread is running.
#[tokio::test]
async fn resume_rejected_while_running() {
    let (_dir, sock) = tmp_sock();
    let _mock = MockCore::start(&sock).await;
    let rt = connect(&sock).await;

    rt.submit("go".into()).await.unwrap();
    assert!(rt.snapshot().running);
    let err = rt.resume_chat("th_other".into()).await.unwrap_err();
    assert!(err.to_string().contains("cannot resume"), "{err}");
}

// inspect_context returns nothing before any cycle has produced a cycle id.
#[tokio::test]
async fn inspect_context_without_cycle_is_empty() {
    let (_dir, sock) = tmp_sock();
    let _mock = MockCore::start(&sock).await;
    let rt = connect(&sock).await;

    let items = rt.inspect_context().await.unwrap();
    assert!(items.is_empty());
}

// shutdown flips the closed flag so the stream reads as stalled.
#[tokio::test]
async fn shutdown_marks_stream_stalled() {
    let (_dir, sock) = tmp_sock();
    let _mock = MockCore::start(&sock).await;
    let rt = connect(&sock).await;

    rt.shutdown().await.unwrap();
    assert!(matches!(rt.stream_state(), Some(StreamState::Stalled)));
}

// A failing worker RPC surfaces the error from worker_op.
#[tokio::test]
async fn worker_op_surfaces_rpc_error() {
    let (_dir, sock) = tmp_sock();
    let cfg = MockCoreConfig::default().with_error("worker.add", "worker.rejected");
    let _mock = MockCore::start_with(&sock, cfg).await;
    let rt = connect(&sock).await;

    let err = rt
        .worker_op(WorkerOp::Add {
            address: Some("addr".into()),
            handle: None,
            label: None,
            harness: None,
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("worker.rejected"), "{err}");
    assert!(
        rt.workers().is_empty(),
        "failed add leaves the registry empty"
    );
}

// An event addressed to an untracked thread is ignored; a valid one still folds.
#[tokio::test]
async fn event_for_untracked_thread_is_ignored() {
    let (_dir, sock) = tmp_sock();
    let mock = MockCore::start(&sock).await;
    let rt = connect(&sock).await;

    let cyc = "cyc:app:th_test:1";
    mock.push_event_for(
        "th_ghost",
        1,
        cyc,
        json!({"kind":"assistant","body":"phantom"}),
    );
    mock.push_event(1, cyc, json!({"kind":"assistant","body":"real"}));

    wait_until("the tracked event folds", T, || {
        rt.snapshot().messages.iter().any(|m| m.content == "real")
    })
    .await;
    assert!(
        !rt.snapshot()
            .messages
            .iter()
            .any(|m| m.content == "phantom"),
        "the ghost-thread event must be dropped"
    );
}

// A seq gap triggers a snapshot.get whose chat rebuilds the message transcript.
#[tokio::test]
async fn resync_rebuilds_chat_from_snapshot() {
    let (_dir, sock) = tmp_sock();
    let mut cfg = MockCoreConfig::default();
    cfg.responses.insert(
        "snapshot.get".into(),
        json!({
            "snapshot": {
                "at": 1,
                "chat": [
                    {"role":"user","body":"rebuilt q"},
                    {"role":"assistant","body":"rebuilt a"}
                ],
                "tasks": [],
            }
        }),
    );
    let mock = MockCore::start_with(&sock, cfg).await;
    let rt = connect(&sock).await;

    let cyc = "cyc:app:th_test:1";
    mock.push_event(1, cyc, json!({"kind":"cycle_start","cycleId":cyc}));
    // Jump the seq to force a gap → snapshot.get → chat rebuild.
    mock.push_event(9, cyc, json!({"kind":"assistant","body":"late"}));

    wait_until("chat rebuilt from the resync snapshot", T, || {
        rt.snapshot()
            .messages
            .iter()
            .any(|m| m.content == "rebuilt a")
    })
    .await;
    let snap = rt.snapshot();
    assert!(snap.messages.iter().any(|m| m.content == "rebuilt q"));
}
