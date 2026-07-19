//! Additional end-to-end tests for [`BackendRuntime`] covering the thread-fleet
//! surface the core scenarios in `e2e_backend.rs` don't reach: fork divergence,
//! `new_session`, async-mode toggle, context inspection, thread summaries, and
//! shutdown. Same in-test [`support::mock_backend`] stub, no real network.

#[path = "../../sdk/tests/support/mod.rs"]
mod support;

use std::time::Duration;

use serde_json::json;

use medulla::client::MedullaClient;
use medulla::runtime::backend::BackendRuntime;
use medulla::runtime::Runtime;

use support::mock_backend::{MockBackend, MockConfig};
use support::wait_until;

const T: Duration = Duration::from_secs(5);

async fn connect(mock: &MockBackend) -> BackendRuntime {
    let client = MedullaClient::new(&mock.base_url, "jwt-test");
    let rt = BackendRuntime::connect(client).await.expect("connect");
    wait_until("initial stream connects", T, || {
        mock.stream_connections() >= 1
    })
    .await;
    rt
}

// Fork copies the parent transcript locally, opens a fresh backend session, and
// then diverges: an event on the fork's own session lands only on the fork.
#[tokio::test]
async fn fork_copies_transcript_then_diverges() {
    let config = MockConfig {
        unique_sessions: true,
        ..MockConfig::default()
    };
    let mock = MockBackend::start_with(config).await;
    let rt = connect(&mock).await;

    // Seed the parent with one turn.
    rt.submit("parent q".into()).await.expect("submit");
    let parent_session = rt.snapshot().session_id.clone();
    mock.emit(
        2,
        &parent_session,
        json!({"kind": "assistant", "body": "parent a"}),
    );
    wait_until("parent turn folds", T, || rt.snapshot().messages.len() == 2).await;

    // Fork: the child becomes active with the copied transcript.
    let child_id = rt.fork(Some("branch".into()));
    let snap = rt.snapshot();
    assert_eq!(snap.active_thread_id, child_id);
    assert_eq!(snap.messages.len(), 2, "transcript copied into the fork");
    assert_eq!(snap.threads.len(), 2);

    // The fork gets its own distinct session.
    wait_until("fork session created", T, || {
        let s = rt.snapshot();
        !s.session_id.is_empty() && s.session_id != parent_session
    })
    .await;
    // A second (and third) stream attach: parent + fork.
    wait_until("fork stream attaches", T, || mock.stream_connections() >= 2).await;

    let child_session = rt.snapshot().session_id.clone();
    mock.emit(
        1,
        &child_session,
        json!({"kind": "assistant", "body": "child a"}),
    );
    wait_until("child event folds only on the fork", T, || {
        rt.snapshot().messages.len() == 3
    })
    .await;
    let snap = rt.snapshot();
    assert_eq!(snap.messages[2].content, "child a");

    // Switching back to the parent shows its (unchanged) transcript.
    rt.set_active_thread("t1".into());
    let parent = rt.snapshot();
    assert_eq!(parent.session_id, parent_session);
    assert_eq!(
        parent.messages.len(),
        2,
        "parent did not see the child event"
    );
}

// new_session resets the active thread and rebinds it to a fresh backend session.
#[tokio::test]
async fn new_session_resets_and_rebinds() {
    let config = MockConfig {
        unique_sessions: true,
        ..MockConfig::default()
    };
    let mock = MockBackend::start_with(config).await;
    let rt = connect(&mock).await;

    let first_session = rt.snapshot().session_id.clone();
    rt.submit("hello".into()).await.expect("submit");
    mock.emit(
        2,
        &first_session,
        json!({"kind": "assistant", "body": "hi"}),
    );
    wait_until("first turn folds", T, || rt.snapshot().messages.len() == 2).await;

    rt.new_session();
    wait_until("new session bound and reset", T, || {
        let s = rt.snapshot();
        !s.session_id.is_empty() && s.session_id != first_session && s.messages.is_empty()
    })
    .await;
    let snap = rt.snapshot();
    assert!(!snap.running);
    assert!(snap.events.is_empty());
}

// The async-mode flag is local-only but is reflected in the snapshot; context
// inspection is empty over HTTP; shutdown resolves cleanly.
#[tokio::test]
async fn async_mode_context_and_shutdown() {
    let mock = MockBackend::start().await;
    let rt = connect(&mock).await;

    assert!(!rt.snapshot().async_mode);
    assert!(rt.set_async_mode(true));
    assert!(rt.snapshot().async_mode);

    let ctx = rt.inspect_context().await.expect("inspect");
    assert!(ctx.is_empty(), "backend exposes no context over HTTP");

    rt.shutdown().await.expect("shutdown");
}

// An error event increments the thread's attention counter surfaced in summaries.
#[tokio::test]
async fn error_events_raise_thread_attention() {
    let mock = MockBackend::start().await;
    let rt = connect(&mock).await;

    rt.submit("work".into()).await.expect("submit");
    mock.emit(2, "sess-1", json!({"kind": "cycle_start", "cycleId": "c1"}));
    mock.emit(
        3,
        "sess-1",
        json!({"kind": "error", "source": "cycle", "message": "oops"}),
    );
    wait_until("attention rises", T, || {
        rt.snapshot()
            .threads
            .iter()
            .any(|t| t.id == "t1" && t.attention >= 1)
    })
    .await;
    let t = rt
        .snapshot()
        .threads
        .into_iter()
        .find(|t| t.id == "t1")
        .unwrap();
    assert_eq!(t.attention, 1);
}
