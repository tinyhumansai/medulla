//! Unit tests for the scripted mock runtime: demo population, thread forking,
//! the submit/abort/session lifecycle, and change
//! notifications.

use super::*;
use crate::runtime::Runtime;
use crate::ui::events::TuiEvent;

#[test]
fn demo_snapshot_is_populated() {
    let rt = MockRuntime::demo();
    let snap = rt.snapshot();
    assert!(!snap.events.is_empty());
    assert_eq!(snap.roster.len(), 1);
    assert!(snap.last_result.is_some());
    assert_eq!(snap.messages.len(), 2);
}

#[tokio::test]
async fn submit_appends_turns() {
    let rt = MockRuntime::empty();
    rt.submit("hello".into()).await.unwrap();
    let snap = rt.snapshot();
    assert!(!snap.running);
    assert_eq!(snap.messages.len(), 2);
    assert!(crate::ui::events::last_assistant_message(&snap.chat_events).is_some());
    assert!(rt.recorded_calls().contains(&"submit".to_string()));
}

#[tokio::test]
async fn submit_rejects_while_running() {
    let rt = MockRuntime::empty();
    rt.set_running(true);
    let err = rt.submit("hi".into()).await.unwrap_err();
    assert!(err.to_string().contains("already running"));
}

#[test]
fn abort_emits_error_only_when_running() {
    let rt = MockRuntime::empty();
    // Idle abort records the call but emits nothing.
    rt.abort();
    assert!(rt.snapshot().events.is_empty());
    // While running, abort emits an operator error and clears the flag.
    rt.set_running(true);
    rt.abort();
    let snap = rt.snapshot();
    assert!(!snap.running);
    assert!(snap
        .events
        .iter()
        .any(|e| matches!(&e.event, TuiEvent::Error { source, .. } if source == "operator")));
    let calls = rt.recorded_calls();
    assert_eq!(calls.iter().filter(|c| *c == "abort").count(), 2);
}

#[test]
fn new_session_clears_history_and_resets_id() {
    let rt = MockRuntime::demo();
    rt.new_session();
    let snap = rt.snapshot();
    assert!(snap.events.is_empty());
    assert!(snap.messages.is_empty());
    assert!(snap.last_result.is_none());
    assert!(!snap.running);
    // A fresh session id is (re)assigned; the clock-derived id may collide within
    // the same millisecond, so we only assert it is non-empty here.
    assert!(!snap.session_id.is_empty());
    assert!(rt.recorded_calls().contains(&"new_session".to_string()));
}

#[test]
fn set_active_thread_ignores_unknown_ids() {
    let rt = MockRuntime::demo();
    assert_eq!(rt.snapshot().active_thread_id, "t1");
    // An unknown id is a no-op; the active thread stays put.
    rt.set_active_thread("nope".into());
    assert_eq!(rt.snapshot().active_thread_id, "t1");
}

#[tokio::test]
async fn resume_chat_appends_and_rejects_while_running() {
    let rt = MockRuntime::empty();
    rt.resume_chat("main".into()).await.unwrap();
    assert_eq!(rt.snapshot().messages.len(), 1);
    rt.set_running(true);
    let err = rt.resume_chat("main".into()).await.unwrap_err();
    assert!(err.to_string().contains("cannot resume"));
}

#[tokio::test]
async fn list_main_chats_and_inspect_context_populate() {
    let rt = MockRuntime::demo();
    let chats = rt.list_main_chats().await.unwrap();
    assert_eq!(chats.len(), 2);
    assert_eq!(chats[0].name, "Auth refactor");
    let ctx = rt.inspect_context().await.unwrap();
    assert_eq!(ctx.len(), 2);
    assert_eq!(ctx[0].kind, "task-result");
}

#[tokio::test]
async fn shutdown_is_ok() {
    let rt = MockRuntime::empty();
    rt.shutdown().await.unwrap();
}

#[test]
fn thread_summaries_count_running_tasks_and_attention() {
    let rt = MockRuntime::empty();
    rt.script_event(TuiEvent::TaskStart {
        task_id: "t1".into(),
        instruction: "go".into(),
        depth: 2,
        agent_id: None,
        contract: None,
    });
    rt.script_event(TuiEvent::TaskAttention {
        task_id: "t1".into(),
        reason: "confirm".into(),
        content: "?".into(),
        question_id: Some("q".into()),
    });
    rt.script_event(TuiEvent::Error {
        source: "cycle".into(),
        message: "oops".into(),
    });
    let snap = rt.snapshot();
    let main = &snap.threads[0];
    assert_eq!(main.running_tasks, 1, "one open task");
    assert_eq!(main.attention, 2, "attention + error both count");
}

#[test]
fn subscribe_receives_a_ping_on_mutation() {
    let rt = MockRuntime::empty();
    let mut rx = rt.subscribe();
    rt.new_session();
    assert!(rx.try_recv().is_ok());
}
