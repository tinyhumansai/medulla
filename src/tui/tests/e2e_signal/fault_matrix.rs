//! Fault matrix: SDK-level resilience under a hostile transport — corrupted
//! bundles that self-heal, transient 5xx tolerated by retry-with-backoff,
//! dropped bundles surfacing non-session errors, and duplicate / out-of-order /
//! duplicate-task-frame delivery that never double-delivers or double-runs.

use std::collections::HashMap;
use std::sync::Arc;

use medulla::daemon::providers::{RunTaskFn, RunTaskOptions, RunTaskResult};
use medulla::daemon::DaemonRuntime;
use medulla::tinyplace::{HarnessProvider, TaskFrame, TaskFrameKind};
use tokio::sync::{mpsc, Notify};

use crate::helpers::*;
use crate::mock_signal_server::MockSignalServer;

// 5a. A corrupted/re-published bundle triggers the SDK session self-heal: the
// first encrypt rejects the tampered signed pre-key (a session-shaped error), the
// retry re-fetches a valid bundle, and the message still delivers.
#[tokio::test]
async fn fault_corrupted_bundle_self_heals() {
    let server = MockSignalServer::start().await;
    let owner = make_identity("owner-heal", &server.base_url);
    let worker = make_identity("worker-heal", &server.base_url);
    owner.transport.publish_keys(&owner.signer).await.unwrap();
    worker.transport.publish_keys(&worker.signer).await.unwrap();

    server.controls().corrupt_next_bundle();
    owner
        .transport
        .send(&worker.id(), "resilient-payload")
        .await
        .unwrap();

    let inbox = worker.transport.drain_inbox(50).await;
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].text, "resilient-payload");
    // The self-heal re-fetched a bundle (two fetches: rejected + valid).
    assert!(server.controls().bundle_fetches() >= 2);
}

// 5b. A transient 5xx on `GET /messages` is tolerated by the SDK's retry-with-
// backoff: the two failing attempts are retried inside one `list()` call, the
// third succeeds, and the message still delivers in a single drain. A longer
// outage that exhausts the retry budget yields an empty drain, and the next
// drain (server recovered) delivers it.
#[tokio::test]
async fn fault_5xx_on_list_tolerated_then_delivers() {
    let server = MockSignalServer::start().await;
    let owner = make_identity("owner-5xx", &server.base_url);
    let worker = make_identity("worker-5xx", &server.base_url);
    owner.transport.publish_keys(&owner.signer).await.unwrap();
    worker.transport.publish_keys(&worker.signer).await.unwrap();
    let worker_id = worker.id();

    // Transient outage within the SDK retry budget (GET retries twice): the
    // single drain still delivers, having retried through the 500s.
    owner
        .transport
        .send(&worker_id, "after-retry")
        .await
        .unwrap();
    server.controls().fail_list(2);
    let calls_before = server.controls().list_calls();
    let inbox = worker.transport.drain_inbox(50).await;
    assert_eq!(inbox.len(), 1, "SDK retry delivered through the 5xx");
    assert_eq!(inbox[0].text, "after-retry");
    assert!(
        server.controls().list_calls() - calls_before >= 3,
        "the list call was retried through the 5xx"
    );

    // A longer outage (exceeds the retry budget) yields an empty drain; the next
    // drain, after recovery, delivers the message.
    owner
        .transport
        .send(&worker_id, "after-the-outage")
        .await
        .unwrap();
    server.controls().fail_list(10);
    let inbox = worker.transport.drain_inbox(50).await;
    assert!(
        inbox.is_empty(),
        "an exhausted retry budget yields an empty drain"
    );
    assert_eq!(server.controls().queued_for(&worker_id), 1);
    // Clear the remaining armed failures, then drain succeeds.
    server.controls().fail_list(0);
    let inbox = worker.transport.drain_inbox(50).await;
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].text, "after-the-outage");
}

// 5 (drop bundle). A dropped bundle (404) is NOT a self-healable session error:
// the send surfaces a plain transport error rather than looping.
#[tokio::test]
async fn fault_dropped_bundle_surfaces_non_session_error() {
    use medulla::daemon::transport::is_session_error;

    let server = MockSignalServer::start().await;
    let owner = make_identity("owner-drop", &server.base_url);
    let worker = make_identity("worker-drop", &server.base_url);
    owner.transport.publish_keys(&owner.signer).await.unwrap();
    worker.transport.publish_keys(&worker.signer).await.unwrap();

    // Drop the next few bundle fetches (covers the self-heal retry too).
    server.controls().drop_next_bundle(3);
    let err = owner
        .transport
        .send(&worker.id(), "never-arrives")
        .await
        .unwrap_err();
    assert!(
        !is_session_error(&err),
        "a 404 bundle is not self-healable: {err}"
    );
}

// 5 (duplicate delivery). Once a session is established, duplicate delivery of an
// envelope does not double-deliver: the second copy fails to decrypt (its ratchet
// slot is spent) and is skipped, so the drain yields exactly one message.
#[tokio::test]
async fn fault_duplicate_delivery_transport_dedupes() {
    let server = MockSignalServer::start().await;
    let owner = make_identity("owner-dup", &server.base_url);
    let worker = make_identity("worker-dup", &server.base_url);
    owner.transport.publish_keys(&owner.signer).await.unwrap();
    worker.transport.publish_keys(&worker.signer).await.unwrap();
    let worker_id = worker.id();

    // Establish the session with a first (prekey-bundle) message.
    owner.transport.send(&worker_id, "establish").await.unwrap();
    assert_eq!(worker.transport.drain_inbox(50).await.len(), 1);

    // A subsequent ciphertext message, delivered twice.
    owner.transport.send(&worker_id, "dup-me").await.unwrap();
    server.controls().set_duplicate_delivery(true);
    let inbox = worker.transport.drain_inbox(50).await;
    assert_eq!(
        inbox.len(),
        1,
        "duplicate delivery yields one decrypted message"
    );
    assert_eq!(inbox[0].text, "dup-me");
    assert_eq!(
        server.controls().queued_for(&worker_id),
        0,
        "ack drained the queue"
    );
}

// 5 (out-of-order delivery). Two in-chain messages delivered in reverse order
// both decrypt: the double-ratchet's skipped-key mechanism reorders them.
#[tokio::test]
async fn fault_out_of_order_delivery_still_decrypts() {
    let server = MockSignalServer::start().await;
    let owner = make_identity("owner-ooo", &server.base_url);
    let worker = make_identity("worker-ooo", &server.base_url);
    owner.transport.publish_keys(&owner.signer).await.unwrap();
    worker.transport.publish_keys(&worker.signer).await.unwrap();
    let worker_id = worker.id();

    // Establish the session first so both reordered messages are plain ciphertext.
    owner.transport.send(&worker_id, "establish").await.unwrap();
    assert_eq!(worker.transport.drain_inbox(50).await.len(), 1);

    owner.transport.send(&worker_id, "message-A").await.unwrap();
    owner.transport.send(&worker_id, "message-B").await.unwrap();
    server.controls().set_out_of_order(true);

    let inbox = worker.transport.drain_inbox(50).await;
    let texts: Vec<&str> = inbox.iter().map(|m| m.text.as_str()).collect();
    assert_eq!(inbox.len(), 2, "both reordered messages decrypt: {texts:?}");
    assert!(texts.contains(&"message-A"));
    assert!(texts.contains(&"message-B"));
}

/// A runner that signals readiness, then blocks on `gate` before replying.
fn blocking_runner(ready: mpsc::UnboundedSender<()>, gate: Arc<Notify>) -> RunTaskFn {
    Arc::new(move |opts: RunTaskOptions| {
        let ready = ready.clone();
        let gate = gate.clone();
        Box::pin(async move {
            let _ = ready.send(());
            gate.notified().await;
            Ok(RunTaskResult {
                usage: None,
                provider: opts.provider,
                reply: "done".to_string(),
                events: 0,
            })
        })
    })
}

// 5 (taskKey dedupe). A duplicate task frame (same sender + taskId) delivered as
// two separate encrypted envelopes does not double-run: the daemon rejects the
// second with "already running" while the first is still executing.
#[tokio::test]
async fn fault_duplicate_task_frame_no_double_run() {
    let server = MockSignalServer::start().await;
    let owner = make_identity("owner-ddup", &server.base_url);
    let worker = make_identity("worker-ddup", &server.base_url);
    owner.transport.publish_keys(&owner.signer).await.unwrap();
    worker.transport.publish_keys(&worker.signer).await.unwrap();
    let worker_id = worker.id();

    let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Notify::new());
    let runtime = DaemonRuntime::new(
        daemon_config(HarnessProvider::Claude, ".".to_string(), HashMap::new()),
        blocking_runner(ready_tx, gate.clone()),
        transport_send(worker.transport.clone()),
    );

    let dup = task_frame(TaskFrameKind::Task, "dup-1", "one", Some("c-dup"));
    let mut collected: Vec<TaskFrame> = Vec::new();

    // First copy: admitted, runs (blocks). Wait for its ack + the runner's ready.
    owner.transport.send(&worker_id, &dup).await.unwrap();
    let admitted = run_chain_until(
        &worker.transport,
        &owner.transport,
        &runtime,
        &mut collected,
        T,
        |frames| {
            frames
                .iter()
                .any(|f| f.kind == TaskFrameKind::Ack && f.text == "task accepted")
        },
    )
    .await;
    assert!(admitted, "first task never admitted: {collected:?}");
    tokio::time::timeout(T, ready_rx.recv()).await.unwrap();

    // Second copy of the same frame while the first is still running → rejected.
    owner.transport.send(&worker_id, &dup).await.unwrap();
    let rejected = run_chain_until(
        &worker.transport,
        &owner.transport,
        &runtime,
        &mut collected,
        T,
        |frames| {
            frames
                .iter()
                .any(|f| f.kind == TaskFrameKind::Error && f.text.contains("already running"))
        },
    )
    .await;
    assert!(rejected, "duplicate task was not rejected: {collected:?}");

    // Release the first task and let it settle.
    gate.notify_waiters();
    runtime.idle().await;
    pump_chain(
        &worker.transport,
        &owner.transport,
        &runtime,
        &mut collected,
    )
    .await;

    // Exactly one admission, one duplicate rejection, and one reply.
    let accepts = collected
        .iter()
        .filter(|f| f.kind == TaskFrameKind::Ack && f.text == "task accepted")
        .count();
    let already = collected
        .iter()
        .filter(|f| f.kind == TaskFrameKind::Error && f.text.contains("already running"))
        .count();
    let replies = collected
        .iter()
        .filter(|f| f.kind == TaskFrameKind::Reply)
        .count();
    assert_eq!(accepts, 1, "task admitted exactly once: {collected:?}");
    assert_eq!(already, 1, "duplicate rejected exactly once: {collected:?}");
    assert_eq!(replies, 1, "the task replied exactly once: {collected:?}");
}
