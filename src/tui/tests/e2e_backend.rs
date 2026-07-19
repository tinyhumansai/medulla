//! End-to-end tests for [`BackendRuntime`] against an in-test mock backend
//! (HTTP + SSE). No real network, no real Medulla server: [`support::mock_backend`]
//! stands in, scripting the SSE body per scenario.

#[path = "../../sdk/tests/support/mod.rs"]
mod support;

use std::time::Duration;

use serde_json::json;

use medulla::client::MedullaClient;
use medulla::runtime::backend::BackendRuntime;
use medulla::runtime::{Runtime, RuntimeSnapshot};
use medulla_tui::ui::events::TuiEvent;

use support::mock_backend::{MockBackend, MockConfig};
use support::wait_until;

const T: Duration = Duration::from_secs(5);

/// Connect a runtime to a fresh mock and wait for its initial stream to attach.
async fn connect(mock: &MockBackend) -> BackendRuntime {
    let client = MedullaClient::new(&mock.base_url, "jwt-test");
    let rt = BackendRuntime::connect(client).await.expect("connect");
    wait_until("initial stream connects", T, || {
        mock.stream_connections() >= 1
    })
    .await;
    rt
}

fn chat_bodies(snap: &RuntimeSnapshot) -> Vec<(String, String)> {
    snap.chat_events
        .iter()
        .filter_map(|env| match &env.event {
            TuiEvent::User { body } => Some(("user".to_string(), body.clone())),
            TuiEvent::Assistant { body } => Some(("assistant".to_string(), body.clone())),
            TuiEvent::Error { source, message } => {
                Some((format!("error:{source}"), message.clone()))
            }
            _ => None,
        })
        .collect()
}

// 1. Full chat round trip.
#[tokio::test]
async fn full_chat_round_trip() {
    let mock = MockBackend::start().await;
    let rt = connect(&mock).await;

    rt.submit("hello".into()).await.expect("submit ok");
    // Optimistic user echo + running toggled on immediately.
    assert!(rt.snapshot().running, "running should be true after submit");

    mock.emit_ping();
    mock.emit(2, "sess-1", json!({"kind": "cycle_start", "cycleId": "c1"}));
    mock.emit(
        3,
        "sess-1",
        json!({"kind": "assistant", "body": "hi there"}),
    );
    mock.emit(
        4,
        "sess-1",
        json!({"kind": "cycle_end", "cycleId": "c1", "passCount": 2, "durationMs": 50}),
    );

    wait_until("cycle end folds", T, || {
        let s = rt.snapshot();
        !s.running && s.last_result.is_some()
    })
    .await;

    let snap = rt.snapshot();
    assert!(!snap.running, "running toggles true → false");
    let lr = snap.last_result.as_ref().unwrap();
    assert_eq!(lr.pass_count, 2);

    let chat = chat_bodies(&snap);
    assert!(
        chat.contains(&("user".into(), "hello".into())),
        "chat: {chat:?}"
    );
    assert!(
        chat.contains(&("assistant".into(), "hi there".into())),
        "chat: {chat:?}"
    );
    // Messages track user + assistant turns.
    assert_eq!(snap.messages.len(), 2);
    assert_eq!(snap.messages[0].role, "user");
    assert_eq!(snap.messages[1].role, "assistant");
}

// 2. Optimistic user echo dedupe: server also replays the user event.
#[tokio::test]
async fn optimistic_user_echo_is_deduped() {
    let mock = MockBackend::start().await;
    let rt = connect(&mock).await;

    rt.submit("hello".into()).await.expect("submit ok");
    // The server replays the user turn AND a distinct assistant reply.
    mock.emit(1, "sess-1", json!({"kind": "user", "body": "hello"}));
    mock.emit(2, "sess-1", json!({"kind": "assistant", "body": "yo"}));

    wait_until("assistant folds", T, || {
        chat_bodies(&rt.snapshot())
            .iter()
            .any(|(r, b)| r == "assistant" && b == "yo")
    })
    .await;

    let snap = rt.snapshot();
    let users: Vec<_> = chat_bodies(&snap)
        .into_iter()
        .filter(|(r, _)| r == "user")
        .collect();
    assert_eq!(
        users.len(),
        1,
        "duplicate user echo must be dropped: {users:?}"
    );
    let user_msgs = snap.messages.iter().filter(|m| m.role == "user").count();
    assert_eq!(user_msgs, 1, "no duplicate user message");
}

// 3. Abort: POST abort reaches the mock and running clears (operator-visible).
#[tokio::test]
async fn abort_hits_backend_and_clears_running() {
    let mock = MockBackend::start().await;
    let rt = connect(&mock).await;

    rt.submit("work".into()).await.expect("submit ok");
    mock.emit(2, "sess-1", json!({"kind": "cycle_start", "cycleId": "c1"}));
    wait_until("running", T, || rt.snapshot().running).await;

    rt.abort();
    let aborted = mock
        .wait_for_request(T, |r| r.method == "POST" && r.path.ends_with("/abort"))
        .await;
    assert!(aborted.path.contains("sess-1"));

    // The abort resolves server-side with a terminal cycle_end → running clears.
    mock.emit(
        3,
        "sess-1",
        json!({"kind": "cycle_end", "cycleId": "c1", "passCount": 0, "durationMs": 1}),
    );
    wait_until("running clears after abort", T, || !rt.snapshot().running).await;
}

// 4. Stream reconnect: server closes mid-cycle, client resumes from Last-Event-ID
//    and delivers the remaining events exactly once.
#[tokio::test]
async fn stream_reconnect_resumes_without_dupes() {
    let mock = MockBackend::start().await;
    let rt = connect(&mock).await;

    rt.submit("work".into()).await.expect("submit ok");
    mock.emit(1, "sess-1", json!({"kind": "cycle_start", "cycleId": "c1"}));
    mock.emit(2, "sess-1", json!({"kind": "assistant", "body": "part1"}));
    wait_until("part1 folds", T, || {
        chat_bodies(&rt.snapshot())
            .iter()
            .any(|(_, b)| b == "part1")
    })
    .await;

    // Drop the connection; the client reconnects (with its cursor) after backoff.
    mock.close_stream();
    wait_until("reconnect", Duration::from_secs(8), || {
        mock.stream_connections() >= 2
    })
    .await;

    // Remaining events on the new connection. The reconnect replays 1&2 (deduped).
    mock.emit(3, "sess-1", json!({"kind": "assistant", "body": "part2"}));
    mock.emit(
        4,
        "sess-1",
        json!({"kind": "cycle_end", "cycleId": "c1", "passCount": 1, "durationMs": 10}),
    );
    wait_until("cycle end after reconnect", T, || !rt.snapshot().running).await;

    let snap = rt.snapshot();
    let assistants: Vec<_> = chat_bodies(&snap)
        .into_iter()
        .filter(|(r, _)| r == "assistant")
        .map(|(_, b)| b)
        .collect();
    assert_eq!(
        assistants,
        vec!["part1", "part2"],
        "each event exactly once"
    );

    // The reconnect carried a Last-Event-ID header.
    let stream_reqs: Vec<_> = mock
        .requests()
        .into_iter()
        .filter(|r| r.path.split('?').next().unwrap_or("").ends_with("/stream"))
        .collect();
    assert!(stream_reqs.len() >= 2, "expected a reconnect");
    assert!(
        stream_reqs.iter().any(|r| r.last_event_id.is_some()),
        "reconnect should send Last-Event-ID: {stream_reqs:?}"
    );
}

// 5. Error cycle: an error event surfaces in chat and running clears.
#[tokio::test]
async fn error_event_surfaces_and_clears_running() {
    let mock = MockBackend::start().await;
    let rt = connect(&mock).await;

    rt.submit("work".into()).await.expect("submit ok");
    mock.emit(2, "sess-1", json!({"kind": "cycle_start", "cycleId": "c1"}));
    mock.emit(
        3,
        "sess-1",
        json!({"kind": "error", "source": "cycle", "message": "blew up"}),
    );
    mock.emit(
        4,
        "sess-1",
        json!({"kind": "cycle_end", "cycleId": "c1", "error": true}),
    );

    wait_until("error folds & running clears", T, || {
        let s = rt.snapshot();
        !s.running && chat_bodies(&s).iter().any(|(r, _)| r == "error:cycle")
    })
    .await;

    let snap = rt.snapshot();
    let err = chat_bodies(&snap)
        .into_iter()
        .find(|(r, _)| r == "error:cycle")
        .expect("error in chat");
    assert_eq!(err.1, "blew up");
    assert!(!snap.running);
}

// 6. resume_chat: session list + message replay rebuild state, then live appends.
#[tokio::test]
async fn resume_chat_rebuilds_then_appends_live() {
    let config = MockConfig {
        sessions_list: json!([{
            "sessionId": "sess-2",
            "title": "Resumed chat",
            "status": "idle",
            "lastSeq": 4,
            "lastActiveAt": 1_700_000_000_000i64,
        }]),
        messages_replay: json!([
            {"seq": 1, "role": "user", "body": "old q", "ts": 1},
            {"seq": 2, "role": "assistant", "body": "old a", "ts": 2},
        ]),
        session_event_seq: 2,
        ..MockConfig::default()
    };
    let mock = MockBackend::start_with(config).await;
    let rt = connect(&mock).await;

    let rows = rt.list_main_chats().await.expect("list");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].session_id, "sess-2");
    assert_eq!(rows[0].name, "Resumed chat");

    let streams_before = mock.stream_connections();
    rt.resume_chat("sess-2".into()).await.expect("resume");

    let snap = rt.snapshot();
    assert_eq!(snap.session_id, "sess-2");
    assert_eq!(snap.messages.len(), 2);
    assert_eq!(snap.messages[0].content, "old q");
    assert_eq!(snap.messages[1].content, "old a");

    // A fresh stream attaches for the resumed session; a live event appends.
    wait_until("resumed stream attaches", T, || {
        mock.stream_connections() > streams_before
    })
    .await;
    mock.emit(
        3,
        "sess-2",
        json!({"kind": "assistant", "body": "new answer"}),
    );

    wait_until("live event appends after resume", T, || {
        rt.snapshot().messages.len() == 3
    })
    .await;
    let snap = rt.snapshot();
    assert_eq!(snap.messages[2].content, "new answer");
    assert!(chat_bodies(&snap).iter().any(|(_, b)| b == "new answer"));
}

// 7. Send failure: 500 on POST messages folds an error and clears running.
#[tokio::test]
async fn send_failure_folds_error() {
    let config = MockConfig {
        messages_ok: false,
        ..MockConfig::default()
    };
    let mock = MockBackend::start_with(config).await;
    let rt = connect(&mock).await;

    let result = rt.submit("hello".into()).await;
    assert!(result.is_err(), "submit should surface the 500");

    wait_until("error folded", T, || {
        let s = rt.snapshot();
        !s.running && chat_bodies(&s).iter().any(|(r, _)| r == "error:cycle")
    })
    .await;

    let snap = rt.snapshot();
    assert!(!snap.running, "running cleared after send failure");
    assert!(chat_bodies(&snap).iter().any(|(r, _)| r == "error:cycle"));
}
