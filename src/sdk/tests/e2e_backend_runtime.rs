//! Mocked end-to-end coverage for [`BackendRuntime`]'s `Runtime` surface — the
//! thread/session bookkeeping and worker plumbing the TUI drives on the hosted
//! path.
//!
//! `BackendRuntime` is now the *only* real runtime, so these pin the parts the
//! feedback suite does not touch: the snapshot contract, fork/active-thread
//! bookkeeping, async-mode toggling, session lifecycle, and the worker seam that
//! answers from the orchestrator hub (empty, not an error, when no hub is
//! attached).

use medulla::client::MedullaClient;
use medulla::runtime::backend::BackendRuntime;
use medulla::runtime::{Runtime, WorkerOp};

#[path = "support/mod.rs"]
mod support;
use support::mock_backend::MockBackend;

async fn runtime(backend: &MockBackend) -> BackendRuntime {
    BackendRuntime::connect(MedullaClient::new(backend.base_url.clone(), "test-jwt"))
        .await
        .expect("the mock serves session creation")
}

#[tokio::test]
async fn connects_with_one_active_thread_and_describes_itself() {
    let backend = MockBackend::start().await;
    let runtime = runtime(&backend).await;

    // Eager session creation means a thread always has a session to stream.
    let snap = runtime.snapshot();
    assert_eq!(snap.threads.len(), 1);
    assert_eq!(snap.active_thread_id, "t1");
    assert!(!snap.session_id.is_empty());
    assert!(!snap.running);
    assert!(!snap.async_mode);

    assert!(!runtime.describe().is_empty());
}

#[tokio::test]
async fn forking_adds_a_thread_and_switching_changes_the_active_one() {
    let backend = MockBackend::start().await;
    let runtime = runtime(&backend).await;

    let forked = runtime.fork(Some("review".to_string()));
    assert_ne!(forked, "t1");

    let snap = runtime.snapshot();
    assert_eq!(snap.threads.len(), 2);
    assert!(snap.threads.iter().any(|t| t.id == forked));
    // A fork takes focus immediately — the backend has no fork primitive, so the
    // child is a local branch the user is dropped into.
    assert_eq!(snap.active_thread_id, forked);

    // Switching back to the parent works...
    runtime.set_active_thread("t1".to_string());
    assert_eq!(runtime.snapshot().active_thread_id, "t1");

    // ...while an unknown id is ignored rather than clearing the selection.
    runtime.set_active_thread("nope".to_string());
    assert_eq!(runtime.snapshot().active_thread_id, "t1");
}

#[tokio::test]
async fn async_mode_toggles_and_is_reflected_in_the_snapshot() {
    let backend = MockBackend::start().await;
    let runtime = runtime(&backend).await;

    assert!(runtime.set_async_mode(true));
    assert!(runtime.snapshot().async_mode);
    assert!(!runtime.set_async_mode(false));
    assert!(!runtime.snapshot().async_mode);
}

#[tokio::test]
async fn a_new_session_replaces_the_active_thread_session() {
    let backend = MockBackend::start().await;
    let runtime = runtime(&backend).await;
    let before = runtime.snapshot().session_id;
    assert!(!before.is_empty());

    // The id is cleared synchronously and refilled by a spawned task, so the
    // thread is briefly session-less; wait for the replacement to land.
    runtime.new_session();
    let mut after = String::new();
    for _ in 0..80 {
        after = runtime.snapshot().session_id;
        if !after.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(
        !after.is_empty(),
        "a new session id should replace the old one"
    );
    // The thread survives the swap.
    assert_eq!(runtime.snapshot().threads.len(), 1);
}

#[tokio::test]
async fn workers_are_empty_without_a_hub_and_ops_do_not_error() {
    // `workers()`/`worker_op()` answer from the orchestrator hub. With no hub
    // attached the roster is empty and operations are inert — the TUI's Workers
    // tab must render an empty list, not surface a failure.
    let backend = MockBackend::start().await;
    let runtime = runtime(&backend).await;

    assert!(runtime.workers().is_empty());

    runtime
        .worker_op(WorkerOp::Add {
            address: Some("GRV1worker".to_string()),
            handle: None,
            label: Some("builder".to_string()),
            harness: Some("claude".to_string()),
        })
        .await
        .expect("an add with no hub attached is inert, not an error");

    runtime
        .worker_op(WorkerOp::Select {
            id: "GRV1worker".to_string(),
        })
        .await
        .expect("a select with no hub attached is inert");

    assert!(runtime.workers().is_empty());
}

#[tokio::test]
async fn submitting_posts_the_message_and_abort_is_accepted() {
    let backend = MockBackend::start().await;
    let runtime = runtime(&backend).await;

    runtime
        .submit("audit the repo".to_string())
        .await
        .expect("the mock accepts a message");

    let posted = backend
        .requests()
        .iter()
        .any(|r| r.path.contains("/messages"));
    assert!(posted, "submit should reach the messages endpoint");

    // Abort is fire-and-forget; it must not panic with a cycle in flight.
    runtime.abort();
}

#[tokio::test]
async fn shutdown_is_idempotent() {
    let backend = MockBackend::start().await;
    let runtime = runtime(&backend).await;

    runtime.shutdown().await.expect("first shutdown succeeds");
    runtime
        .shutdown()
        .await
        .expect("a second shutdown is harmless");
}
