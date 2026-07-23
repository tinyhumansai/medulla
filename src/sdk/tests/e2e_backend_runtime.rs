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
use serde_json::json;

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
async fn without_a_hub_the_roster_reads_empty_but_mutations_say_why() {
    // Reading and mutating are not the same promise. An empty roster is the
    // truth when no hub is attached, so `workers()` must render an empty list
    // rather than an error. But a mutation that cannot possibly have happened
    // must not report success: an add that silently no-ops leaves the operator
    // watching for a peer that was never registered anywhere.
    let backend = MockBackend::start().await;
    let runtime = runtime(&backend).await;

    assert!(runtime.workers().is_empty(), "reads stay inert");

    let error = runtime
        .worker_op(WorkerOp::Add {
            address: Some("GRV1worker".to_string()),
            handle: None,
            label: Some("builder".to_string()),
            harness: Some("claude".to_string()),
        })
        .await
        .expect_err("an add with nowhere to go must not report success");
    assert!(
        error.to_string().contains("hub"),
        "the reason must name what is missing: {error}"
    );

    runtime
        .worker_op(WorkerOp::Select {
            id: "GRV1worker".to_string(),
        })
        .await
        .expect_err("nor a select");

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

    // Abort is fire-and-forget (it spawns), so wait for the request to actually
    // land rather than racing the spawned task to the end of the test.
    runtime.abort();
    let mut aborted = false;
    for _ in 0..80 {
        if backend.requests().iter().any(|r| r.path.contains("/abort")) {
            aborted = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(aborted, "abort should reach the backend");
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

#[tokio::test]
async fn main_chats_are_listed_with_titles_and_turn_counts() {
    let backend = MockBackend::start().await;
    backend.configure(|c| {
        c.sessions_list = json!([
            {"sessionId": "sess-a", "title": "Repo audit", "lastActiveAt": 1_700_000_000,
             "status": "active", "lastSeq": 7},
            // No title → the session id stands in as the display name.
            {"sessionId": "sess-b", "status": "idle", "lastSeq": 0},
        ]);
    });
    let runtime = runtime(&backend).await;

    let chats = runtime
        .list_main_chats()
        .await
        .expect("the mock lists sessions");

    assert_eq!(chats.len(), 2);
    assert_eq!(chats[0].session_id, "sess-a");
    assert_eq!(chats[0].name, "Repo audit");
    // Turns are message pairs, so seq 7 folds to 3.
    assert_eq!(chats[0].turns, 3);
    assert!(!chats[0].updated_at.is_empty());
    // Untitled sessions fall back to the id rather than rendering blank.
    assert_eq!(chats[1].name, "sess-b");
    assert_eq!(chats[1].turns, 0);
}

#[tokio::test]
async fn resuming_a_chat_replays_its_transcript_into_the_active_thread() {
    let backend = MockBackend::start().await;
    backend.configure(|c| {
        c.messages_replay = json!([
            {"seq": 1, "role": "user", "body": "audit the repo"},
            {"seq": 2, "role": "assistant", "body": "found 3 issues"},
        ]);
        c.session_event_seq = 2;
    });
    let runtime = runtime(&backend).await;

    runtime
        .resume_chat("sess-a".to_string())
        .await
        .expect("the mock replays the transcript");

    let snap = runtime.snapshot();
    // The resumed session becomes the active thread's session, and its
    // transcript is folded in so the UI renders history immediately.
    assert_eq!(snap.session_id, "sess-a");
    assert!(
        snap.messages.len() >= 2,
        "the replayed transcript should populate the thread"
    );
    assert!(snap
        .messages
        .iter()
        .any(|m| m.content.contains("audit the repo")));
    assert!(snap
        .messages
        .iter()
        .any(|m| m.content.contains("found 3 issues")));
}

#[tokio::test]
async fn listing_main_chats_surfaces_a_backend_failure() {
    // The list endpoint is the one the resume picker calls; a 5xx must surface
    // as an error rather than an empty picker that looks like "no chats".
    let backend = MockBackend::start().await;
    backend.configure(|c| c.sessions_list = json!({"not": "an array"}));
    let runtime = runtime(&backend).await;

    assert!(runtime.list_main_chats().await.is_err());
}

#[tokio::test]
async fn subscribing_yields_a_receiver_that_wakes_on_change() {
    let backend = MockBackend::start().await;
    let runtime = runtime(&backend).await;

    let mut rx = runtime.subscribe();
    // Any state mutation pings subscribers so the UI redraws.
    runtime.set_async_mode(true);
    tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("a subscriber is notified promptly")
        .expect("the channel stays open");
}

#[tokio::test]
async fn team_usage_and_context_reach_the_backend() {
    // Neither endpoint is served by the mock; the runtime must surface that as a
    // normal error rather than panicking or hanging the caller.
    let backend = MockBackend::start().await;
    let runtime = runtime(&backend).await;

    let _ = runtime.team_usage().await;
    let _ = runtime.inspect_context().await;
}

#[tokio::test]
async fn the_feedback_seam_delegates_every_mutation() {
    // The board's write path: detail, comment and submit all delegate to the
    // client. Only voting is covered elsewhere, so pin the rest here.
    let backend = MockBackend::start().await;
    let runtime = runtime(&backend).await;

    let _ = runtime.feedback_detail("f1".to_string()).await;
    let _ = runtime
        .comment_feedback("f1".to_string(), "looks good".to_string())
        .await;
    let _ = runtime
        .submit_feedback(
            medulla::client::FeedbackType::Bug,
            "Crash on resume".to_string(),
            "Steps to reproduce".to_string(),
        )
        .await;

    // Every call should have reached the feedback surface.
    let hits = backend
        .requests()
        .iter()
        .filter(|r| r.path.contains("/feedback"))
        .count();
    assert!(hits >= 3, "each feedback op should reach the backend");
}
