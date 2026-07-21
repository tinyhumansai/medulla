//! Socket-facing integration tests: these drive a [`StubServer`] over a real
//! unix socket to cover the handshake, instruct round-trip, reconnect/replay,
//! steering hooks, and the local (no-serve-frame) lifecycle hooks.

use std::time::Duration;

use serde_json::json;

use crate::harness_contract::{HarnessState, TrackedTaskStatus};
use crate::runtime::{Runtime, StreamState};

use super::super::client::CoreRuntime;
use super::super::runtime_impl::is_unavailable;
use super::super::stub_server::{StubConfig, StubServer};
use super::wait_until;

#[tokio::test]
async fn handshake_attaches_and_records_identity() {
    let server = StubServer::start(StubConfig::default());
    let rt = CoreRuntime::attach(server.path.clone());

    assert!(
        wait_until(|| rt.stream_state() == Some(StreamState::Live)).await,
        "runtime should reach Live after the handshake"
    );
    let snap = rt.snapshot();
    assert_eq!(snap.session_id, "agent");
    assert!(rt.describe().contains("attached"), "{}", rt.describe());
    assert!(
        rt.describe().contains("3.12.0"),
        "serve version in describe"
    );
    // The stub saw a hello and a subscribe as the first two ops.
    let ops = server.received_ops();
    assert_eq!(ops.first().map(String::as_str), Some("hello"));
    assert!(ops.contains(&"subscribe".to_string()));
}

#[tokio::test]
async fn instruct_round_trips_and_streams_events() {
    let server = StubServer::start(StubConfig::default());
    let rt = CoreRuntime::attach(server.path.clone());
    assert!(wait_until(|| rt.stream_state() == Some(StreamState::Live)).await);

    rt.submit("reconcile the world".into())
        .await
        .expect("instruct should ack");

    // The streamed cycle_start/task_board_changed/cycle_end fold into the snapshot.
    assert!(
        wait_until(|| {
            let s = rt.snapshot();
            !s.running
                && s.harness
                    .as_ref()
                    .map(|h| !h.tasks.is_empty() && h.state == HarnessState::Idle)
                    .unwrap_or(false)
        })
        .await,
        "cycle should settle and the board should carry the task"
    );

    let snap = rt.snapshot();
    // The optimistic user turn plus the streamed cycle framing are all present.
    assert!(snap.messages.iter().any(|m| m.role == "user"));
    let task = &snap.harness.unwrap().tasks[0];
    assert_eq!(task.id, "t1");
    assert_eq!(task.status, TrackedTaskStatus::Active);
    assert!(server.received_ops().contains(&"instruct".to_string()));
}

#[tokio::test]
async fn version_mismatch_marks_unavailable() {
    let server = StubServer::start(StubConfig {
        protocol: 2,
        ..StubConfig::default()
    });
    let rt = CoreRuntime::attach(server.path.clone());

    assert!(
        wait_until(|| is_unavailable(&rt)).await,
        "a protocol mismatch must latch unavailable"
    );
    assert_eq!(rt.stream_state(), Some(StreamState::Stalled));
    assert!(rt.describe().contains("unavailable"), "{}", rt.describe());
    // The host never sent a hello once it saw the bad banner.
    assert!(!server.received_ops().contains(&"hello".to_string()));
}

#[tokio::test]
async fn hello_rejection_marks_unavailable() {
    let server = StubServer::start(StubConfig {
        hello_ok: false,
        ..StubConfig::default()
    });
    let rt = CoreRuntime::attach(server.path.clone());

    assert!(
        wait_until(|| is_unavailable(&rt)).await,
        "a hello rejection must latch unavailable"
    );
    assert!(server.received_ops().contains(&"hello".to_string()));
}

#[tokio::test]
async fn socket_drop_triggers_reconnect() {
    let server = StubServer::start(StubConfig {
        drop_after_instruct: true,
        instruct_events: vec![json!({"kind":"cycle_start","instructionId":"i0","cycleId":"c0"})],
        ..StubConfig::default()
    });
    let rt = CoreRuntime::attach(server.path.clone());
    assert!(wait_until(|| rt.stream_state() == Some(StreamState::Live)).await);

    // First instruct: the stub acks, streams an event, then drops the socket.
    rt.submit("first".into())
        .await
        .expect("first instruct acks");
    assert!(
        wait_until(|| server.accept_count() >= 2).await,
        "the host should re-attach after the drop"
    );

    // The re-attached session still services requests.
    assert!(wait_until(|| rt.stream_state() == Some(StreamState::Live)).await);
    rt.submit("second".into())
        .await
        .expect("second instruct acks on the re-attached session");
}

#[tokio::test]
async fn reconnect_replay_rebaselines_instead_of_double_counting() {
    // The first connection folds two full cycles (usage.cycles == 2), then drops.
    // On re-attach the host sends subscribe{replay}, and the stub replays a single
    // cycle. Because the driver resets the fold-derived state before the replay,
    // the settled state reflects exactly that one replayed cycle (usage.cycles ==
    // 1) rather than stacking it on top of the pre-drop count (which would read 3).
    let server = StubServer::start(StubConfig {
        drop_after_instruct: true,
        instruct_events: vec![
            json!({"kind":"cycle_start","instructionId":"i0","cycleId":"c0"}),
            json!({"kind":"cycle_end","instructionId":"i0","cycleId":"c0"}),
            json!({"kind":"cycle_start","instructionId":"i1","cycleId":"c1"}),
            json!({"kind":"cycle_end","instructionId":"i1","cycleId":"c1"}),
        ],
        replay_events: vec![
            json!({"kind":"cycle_start","instructionId":"i0","cycleId":"c0"}),
            json!({"kind":"task_board_changed","task":{
                "id":"t1","title":"reconcile","status":"active",
                "createdAt":"0","updatedAt":"0","delegatedTaskIds":[],"notes":[]}}),
            json!({"kind":"cycle_end","instructionId":"i0","cycleId":"c0"}),
        ],
        ..StubConfig::default()
    });
    let rt = CoreRuntime::attach(server.path.clone());
    assert!(wait_until(|| rt.stream_state() == Some(StreamState::Live)).await);

    // First instruct: streams two cycles, then the stub drops the socket.
    rt.submit("first".into())
        .await
        .expect("first instruct acks");
    assert!(
        wait_until(|| server.accept_count() >= 2).await,
        "the host should re-attach after the drop"
    );

    // usage.cycles == 1 is only reachable if the replay rebaselined a reset state:
    // pre-drop the counter was 2, and without the reset the replay would drive it
    // to 3 — never back to 1. The board is rebuilt from the replayed task too.
    assert!(
        wait_until(|| {
            let s = rt.snapshot();
            s.harness
                .as_ref()
                .map(|h| {
                    h.usage.cycles == 1
                        && h.queued == 0
                        && h.tasks.len() == 1
                        && h.tasks[0].id == "t1"
                })
                .unwrap_or(false)
        })
        .await,
        "replay should rebaseline usage.cycles to 1, not double-count to 3: {:?}",
        rt.snapshot().harness
    );
}

#[tokio::test]
async fn steering_hooks_reach_serve() {
    let server = StubServer::start(StubConfig::default());
    let rt = CoreRuntime::attach(server.path.clone());
    assert!(wait_until(|| rt.stream_state() == Some(StreamState::Live)).await);

    rt.answer_question("cyc:1".into(), "q7".into(), "yes".into());
    rt.cancel_task("cyc:1".into(), "t3".into());
    rt.abort(); // maps to stop{drain:false}

    assert!(
        wait_until(|| {
            let ops = server.received_ops();
            ops.contains(&"answer_question".to_string())
                && ops.contains(&"cancel_task".to_string())
                && ops.contains(&"stop".to_string())
        })
        .await,
        "answer_question, cancel_task, and stop should all reach serve"
    );
}

#[tokio::test]
async fn local_lifecycle_hooks_behave() {
    let server = StubServer::start(StubConfig::default());
    let rt = CoreRuntime::attach(server.path.clone());
    assert!(wait_until(|| rt.stream_state() == Some(StreamState::Live)).await);
    rt.submit("hi".into()).await.unwrap();
    assert!(wait_until(|| !rt.snapshot().messages.is_empty()).await);

    // set_async_mode is a local echo.
    assert!(rt.set_async_mode(true));
    assert!(rt.snapshot().async_mode);

    // new_session clears the local transcript without touching serve.
    rt.new_session();
    assert!(rt.snapshot().messages.is_empty());

    // fork returns the current session id; set_active_thread is inert.
    assert_eq!(rt.fork(None), "agent");
    rt.set_active_thread("whatever".into());

    // The no-op surfaces resolve cleanly.
    assert!(rt.list_main_chats().await.unwrap().is_empty());
    assert!(rt.inspect_context().await.unwrap().is_empty());
    rt.resume_chat("s".into()).await.unwrap();
    rt.shutdown().await.unwrap();
}

#[tokio::test]
async fn attach_to_missing_socket_stays_reconnecting() {
    // No server: the attach keeps retrying rather than latching unavailable.
    let path = std::env::temp_dir().join(format!("mdl-absent-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let rt = CoreRuntime::attach(path);
    // It never reaches Live and never latches a fatal state.
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_ne!(rt.stream_state(), Some(StreamState::Live));
    assert!(!is_unavailable(&rt));
}

#[tokio::test]
async fn shutdown_does_not_hang_while_reconnecting() {
    // No server is ever started: the driver sits in the connect/backoff loop
    // (attempting `UnixStream::connect` against an absent socket, then
    // sleeping `RECONNECT_DELAY`) exactly like the regression this covers —
    // `shutdown()` must be acked promptly rather than hang behind a driver
    // that's stuck reconnecting (Codex review finding #1).
    let path =
        std::env::temp_dir().join(format!("mdl-absent-shutdown-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let rt = CoreRuntime::attach(path);

    // Give the driver a couple of failed connect/backoff cycles to make sure
    // shutdown really does race an in-flight reconnect attempt, not just a
    // freshly spawned task that hasn't reached the loop yet.
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(!is_unavailable(&rt));
    assert_ne!(rt.stream_state(), Some(StreamState::Live));

    // A regression here hangs forever; bound it so a broken build fails fast
    // instead of stalling CI.
    tokio::time::timeout(Duration::from_secs(5), rt.shutdown())
        .await
        .expect("shutdown() must not hang while the driver is reconnecting")
        .expect("shutdown() should resolve Ok even with no live connection");
}

#[tokio::test]
async fn failed_instruct_surfaces_the_wire_error() {
    let server = StubServer::start(StubConfig {
        instruct_fail: true,
        ..StubConfig::default()
    });
    let rt = CoreRuntime::attach(server.path.clone());
    assert!(wait_until(|| rt.stream_state() == Some(StreamState::Live)).await);

    let err = rt
        .submit("do it".into())
        .await
        .expect_err("a failed instruct must propagate");
    assert!(err.to_string().contains("instruct failed"), "{err}");
}

#[tokio::test]
async fn inbound_port_call_and_late_ready_are_handled() {
    // After hello, the stub pushes a reverse-RPC `call` (refused port_unavailable)
    // and a duplicate `ready` (re-recorded). The session stays Live throughout.
    let server = StubServer::start(StubConfig {
        after_hello: vec![
            json!({"t":"call","id":"c1","port":"inference","method":"invoke","params":{}}),
            json!({"t":"ready","protocol":1,"serve":"3.12.1","sessionId":"agent","error":null}),
        ],
        ..StubConfig::default()
    });
    let rt = CoreRuntime::attach(server.path.clone());
    assert!(wait_until(|| rt.stream_state() == Some(StreamState::Live)).await);
    // A follow-up instruct still round-trips after handling the extra frames.
    rt.submit("ping".into()).await.expect("still serviceable");
    assert!(rt.describe().contains("3.12"), "{}", rt.describe());
}
