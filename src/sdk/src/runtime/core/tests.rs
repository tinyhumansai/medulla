//! Unit + in-crate integration tests for the core (`medulla-serve`) runtime.
//! The socket-facing tests drive a [`StubServer`] over a real unix socket; the
//! pure tests exercise the frame grammar, the event fold, and the state model.

use std::time::Duration;

use serde_json::{json, Value};

use crate::harness_contract::{HarnessState, TrackedTaskStatus};
use crate::runtime::{Runtime, StreamState};

use super::client::CoreRuntime;
use super::protocol::{
    check_ready, fold_event, hello_params, parse_line, port_unavailable_ret, req_line, Inbound,
    ReadyCheck,
};
use super::runtime_impl::is_unavailable;
use super::stub_server::{StubConfig, StubServer};
use super::types::{ConnState, CoreError, CoreState, PROTOCOL_VERSION};

/// Poll `f` up to ~2 s, returning whether it ever held. Keeps the async socket
/// tests deterministic without sleeping a fixed amount.
async fn wait_until<F: Fn() -> bool>(f: F) -> bool {
    for _ in 0..200 {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    f()
}

// --- socket-facing integration tests ---------------------------------------

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

// --- frame grammar (protocol.rs) -------------------------------------------

#[test]
fn parse_line_decodes_each_serve_to_host_frame() {
    match parse_line(
        r#"{"t":"ready","protocol":1,"serve":"3.12.0","sessionId":"agent","error":null}"#,
    ) {
        Some(Inbound::Ready {
            protocol,
            serve,
            session_id,
            error,
        }) => {
            assert_eq!(protocol, 1);
            assert_eq!(serve.as_deref(), Some("3.12.0"));
            assert_eq!(session_id.as_deref(), Some("agent"));
            assert!(error.is_none());
        }
        other => panic!("expected Ready, got {other:?}"),
    }
    match parse_line(r#"{"t":"res","id":"h1","ok":true,"result":{"x":1}}"#) {
        Some(Inbound::Res {
            id,
            ok,
            result,
            error,
        }) => {
            assert_eq!(id, "h1");
            assert!(ok);
            assert_eq!(result, json!({"x":1}));
            assert!(error.is_none());
        }
        other => panic!("expected Res, got {other:?}"),
    }
    match parse_line(
        r#"{"t":"res","id":"2","ok":false,"error":{"code":"timeout","message":"slow"}}"#,
    ) {
        Some(Inbound::Res { ok, error, .. }) => {
            assert!(!ok);
            let e = error.unwrap();
            assert_eq!(e.code, "timeout");
            assert_eq!(e.message, "slow");
        }
        other => panic!("expected failed Res, got {other:?}"),
    }
    match parse_line(r#"{"t":"call","id":"c1","port":"inference","method":"invoke"}"#) {
        Some(Inbound::Call { id, port }) => {
            assert_eq!(id, "c1");
            assert_eq!(port, "inference");
        }
        other => panic!("expected Call, got {other:?}"),
    }
    match parse_line(
        r#"{"t":"event","seq":7,"at":0,"event":{"kind":"cycle_start","instructionId":"i","cycleId":"c"}}"#,
    ) {
        Some(Inbound::Event { seq, event }) => {
            assert_eq!(seq, 7);
            assert_eq!(event.get("kind").unwrap(), "cycle_start");
        }
        other => panic!("expected Event, got {other:?}"),
    }
}

#[test]
fn parse_line_skips_malformed_and_unknown() {
    assert!(parse_line("").is_none());
    assert!(parse_line("   ").is_none());
    assert!(parse_line("not json").is_none());
    assert!(parse_line(r#"{"no":"discriminant"}"#).is_none());
    assert!(parse_line(r#"{"t":"emit","id":"c1"}"#).is_none()); // host→serve, never inbound
    assert!(parse_line(r#"{"t":"res"}"#).is_none()); // missing id
}

#[test]
fn check_ready_flags_mismatch_and_startup_error() {
    assert!(matches!(
        check_ready(
            PROTOCOL_VERSION,
            Some("v".into()),
            Some("agent".into()),
            None
        ),
        ReadyCheck::Ok { .. }
    ));
    assert!(matches!(
        check_ready(2, None, None, None),
        ReadyCheck::Fatal(_)
    ));
    assert!(matches!(
        check_ready(1, None, None, Some("boom".into())),
        ReadyCheck::Fatal(_)
    ));
}

#[test]
fn outbound_frames_are_well_formed() {
    let req = req_line("r1", "instruct", &json!({"message":"hi"}));
    assert!(req.ends_with('\n'));
    let v: Value = serde_json::from_str(req.trim()).unwrap();
    assert_eq!(v["t"], "req");
    assert_eq!(v["op"], "instruct");
    assert_eq!(v["id"], "r1");

    let ret = port_unavailable_ret("c9", "budgets");
    let v: Value = serde_json::from_str(ret.trim()).unwrap();
    assert_eq!(v["t"], "ret");
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["code"], "port_unavailable");

    let hello = hello_params();
    assert_eq!(hello["protocol"], PROTOCOL_VERSION);
    assert!(hello["host"]
        .as_str()
        .unwrap()
        .starts_with("medulla-public/"));
    assert!(hello["ports"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p == "inference"));
}

// --- event fold + state model (protocol.rs / types.rs) ---------------------

#[test]
fn fold_event_drives_running_and_board() {
    let mut s = CoreState::new();

    assert!(fold_event(
        &mut s,
        &json!({"kind":"cycle_start","instructionId":"i","cycleId":"c"})
    ));
    assert!(s.running);
    assert_eq!(s.harness.as_ref().unwrap().state, HarnessState::Running);

    let task = json!({"kind":"task_board_changed","task":{
        "id":"t1","title":"do","status":"open","createdAt":"0","updatedAt":"0",
        "delegatedTaskIds":[],"notes":[]}});
    assert!(fold_event(&mut s, &task));
    assert_eq!(s.harness.as_ref().unwrap().tasks.len(), 1);
    // A second board change with the same id updates in place.
    let task2 = json!({"kind":"task_board_changed","task":{
        "id":"t1","title":"do","status":"done","createdAt":"0","updatedAt":"1",
        "delegatedTaskIds":[],"notes":[]}});
    assert!(fold_event(&mut s, &task2));
    let tasks = &s.harness.as_ref().unwrap().tasks;
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].status, TrackedTaskStatus::Done);

    assert!(fold_event(
        &mut s,
        &json!({"kind":"cycle_end","instructionId":"i","cycleId":"c"})
    ));
    assert!(!s.running);
    assert_eq!(s.harness.as_ref().unwrap().state, HarnessState::Idle);
    assert_eq!(s.harness.as_ref().unwrap().usage.cycles, 1);
    assert!(s.last_result.is_some());
}

#[test]
fn fold_event_passes_unknown_and_cycle_events_through() {
    let mut s = CoreState::new();
    // A serve-level roster_event is not a HarnessEvent: kept verbatim.
    assert!(fold_event(
        &mut s,
        &json!({"kind":"roster_event","agent":{"id":"a"}})
    ));
    // instruction_queued increments the queue counter.
    assert!(fold_event(
        &mut s,
        &json!({"kind":"instruction_queued","instructionId":"i","cycleId":"c"})
    ));
    assert_eq!(s.harness.as_ref().unwrap().queued, 1);
    // A cycle_event wraps an inner cycle event, which rides through as an event row.
    assert!(fold_event(
        &mut s,
        &json!({"kind":"cycle_event","event":{"kind":"inference_end"}})
    ));
    assert!(!s.events.is_empty());
}

#[test]
fn stream_health_maps_conn_and_gap() {
    let mut s = CoreState::new();
    assert_eq!(s.stream_health(), StreamState::Resyncing); // Connecting
    s.conn = ConnState::Live;
    assert_eq!(s.stream_health(), StreamState::Live);

    // A non-contiguous protocol seq latches a gap → Resyncing.
    s.note_stream_seq(1);
    s.note_stream_seq(2);
    assert!(!s.gap);
    s.note_stream_seq(9);
    assert!(s.gap);
    assert_eq!(s.stream_health(), StreamState::Resyncing);

    // A fresh connection resets the cursor.
    s.reset_stream_cursor();
    assert!(!s.gap);
    assert_eq!(s.stream_health(), StreamState::Live);

    s.conn = ConnState::Reconnecting;
    assert_eq!(s.stream_health(), StreamState::Resyncing);
    s.conn = ConnState::Unavailable("boom".into());
    assert_eq!(s.stream_health(), StreamState::Stalled);
}

#[test]
fn describe_reflects_lifecycle_and_error_display() {
    let mut s = CoreState::new();
    assert!(s.describe().contains("connecting"));
    s.serve_version = Some("3.12.0".into());
    s.conn = ConnState::Live;
    assert!(s.describe().contains("3.12.0") && s.describe().contains("attached"));
    s.conn = ConnState::Unavailable("bad".into());
    assert!(s.describe().contains("unavailable: bad"));

    let e = CoreError::transport("dropped");
    assert!(e.to_string().contains("dropped") && e.to_string().contains("internal"));
}

#[test]
fn event_log_and_chat_log_respect_caps_and_chattiness() {
    let mut s = CoreState::new();
    // A chat-visible event lands in both logs; a non-chat event only in events.
    s.emit(crate::ui::events::TuiEvent::User { body: "hi".into() });
    s.emit(crate::ui::events::TuiEvent::CycleStart {
        cycle_id: "c".into(),
    });
    assert_eq!(s.events.len(), 2);
    assert_eq!(s.chat_events.len(), 1);
    assert!(s.events[0].seq < s.events[1].seq);
}
