//! The spawned stream task and [`ScreenRouter`]: who may watch what, and
//! starting or stopping the stream that answers.
//!
//! Async and pty-backed — unlike [`conversion`](super::conversion) and
//! [`sampler`](super::sampler), these drive the real dispatch machinery a
//! subscribing peer goes through.

use crate::worker::stream::sampler;
use crate::worker::stream::{send_fn, spawn_session_stream, ScreenRouter, StreamRegistry};

// --- the spawned task ------------------------------------------------------

#[tokio::test]
async fn a_subscription_ends_when_its_session_does_not_exist() {
    // A stream must not outlive the thing it is watching, and must not sit
    // spinning against a session id that was never there.
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    let sent: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = sent.clone();
    let send = send_fn(move |to, body| {
        let recorder = recorder.clone();
        async move {
            recorder.lock().unwrap().push((to, body));
        }
    });

    let handle = spawn_session_stream(
        crate::worker::pty::PtyManager::new(),
        spec("t1", "w_missing", "peer", 10, always_live()),
        send,
    );

    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("the task must end rather than spin")
        .expect("and end cleanly");
    assert!(
        sent.lock().unwrap().is_empty(),
        "nothing should be sent for a session that does not exist"
    );
}

#[tokio::test]
async fn unsubscribing_stops_the_stream() {
    let sessions = crate::worker::pty::PtyManager::new();
    let send = send_fn(|_, _| async {});
    let mut registry = StreamRegistry::new();

    assert!(registry.is_empty());
    registry.subscribe(
        &sessions,
        spec("t1", "w_1", "peer", 1, always_live()),
        send.clone(),
    );
    assert_eq!(registry.len(), 1);

    // A second subscribe replaces rather than fans out: two watchers on one
    // session would double its transport cost for no new information.
    registry.subscribe(&sessions, spec("t1", "w_1", "peer", 1, always_live()), send);
    assert_eq!(registry.len(), 1);

    registry.unsubscribe("t1");
    assert!(registry.is_empty());
}

// --- the router ------------------------------------------------------------

/// A liveness check that always says yes, for the tests that are about
/// something else.
fn always_live() -> sampler::LiveCheck {
    std::sync::Arc::new(|| true)
}

/// A [`StreamSpec`] from its parts, so the tests read as call sites rather than
/// struct literals.
fn spec(
    task_id: &str,
    session_id: &str,
    subscriber: &str,
    max_fps: u8,
    is_live: sampler::LiveCheck,
) -> sampler::StreamSpec {
    sampler::StreamSpec {
        task_id: task_id.to_string(),
        session_id: session_id.to_string(),
        subscriber: subscriber.to_string(),
        max_fps,
        is_live,
    }
}

/// A router over an empty worker: no sessions, no running tasks.
fn empty_router() -> ScreenRouter {
    let runtime = medulla::daemon::DaemonRuntime::new(
        medulla::daemon::DaemonConfig {
            hooks: medulla::harness_hooks::HooksConfig::default(),
            providers: vec![medulla::protocol::HarnessProvider::Claude],
            default_provider: medulla::protocol::HarnessProvider::Claude,
            workspace: "/tmp".into(),
            env: std::collections::HashMap::new(),
            task_timeout_ms: 1_000,
            capability_timeout_ms: None,
            concurrency: 1,
            status_throttle_ms: 1_000,
            max_pending: 1,
            model: None,
            agent: None,
            extra_args: Vec::new(),
            skip_permissions: false,
            accessible_dirs: Vec::new(),
            router: None,
            custom_harnesses: Vec::new(),
            budget: None,
            attribution: true,
        },
        std::sync::Arc::new(|_| Box::pin(async { Err("unused".to_string()) })),
        std::sync::Arc::new(|_, _| Box::pin(async {})),
    );
    ScreenRouter::new(
        crate::worker::pty::PtyManager::new(),
        runtime,
        send_fn(|_, _| async {}),
    )
}

#[tokio::test]
async fn a_subscribe_for_a_task_this_sender_never_dispatched_is_refused() {
    // Authorization is structural: the running-task record is keyed by
    // (authenticated sender, task id), so a peer cannot name another's task —
    // the key it would need includes an identity it does not have.
    let mut router = empty_router();
    router.handle(
        "peerA",
        medulla::protocol::ScreenMessage::Subscribe {
            task_id: "t1".into(),
            max_fps: 1,
            resync: true,
        },
    );
    assert_eq!(router.active(), 0, "nothing may be streamed");
}

#[tokio::test]
async fn an_unsubscribe_for_a_task_nobody_streams_does_nothing() {
    // Same rule in the other direction: without it, any peer could cancel
    // another's stream by naming its task id.
    let mut router = empty_router();
    router.handle(
        "peerA",
        medulla::protocol::ScreenMessage::Unsubscribe {
            task_id: "t1".into(),
        },
    );
    assert_eq!(router.active(), 0);
}

#[tokio::test]
async fn a_kill_for_a_task_this_sender_never_dispatched_is_refused() {
    let mut router = empty_router();
    router.handle(
        "peerA",
        medulla::protocol::ScreenMessage::Kill {
            task_id: "t1".into(),
            correlation_id: "cyc/t1/0".into(),
        },
    );
    assert_eq!(router.active(), 0);
}

#[tokio::test]
async fn only_the_peer_a_stream_was_opened_for_can_stop_it() {
    let sessions = crate::worker::pty::PtyManager::new();
    let send = send_fn(|_, _| async {});
    let mut registry = StreamRegistry::new();
    registry.subscribe(
        &sessions,
        spec("t1", "w_1", "peerA", 1, always_live()),
        send,
    );
    assert_eq!(registry.len(), 1);

    assert!(
        !registry.unsubscribe_for("peerB", "t1"),
        "a stranger naming the task must not stop it"
    );
    assert_eq!(registry.len(), 1, "and the stream survives");

    assert!(registry.unsubscribe_for("peerA", "t1"));
    assert!(registry.is_empty());
}

#[tokio::test]
async fn a_stream_ends_when_its_task_does_even_though_the_session_lives_on() {
    // The case the old rule got wrong. An interactive session outlives the task
    // that ran in it and is handed to the next one, so a stream that watched
    // only the session would carry on — labelling whatever ran next with the
    // task id it was opened for, and sending it to the peer that asked for the
    // *old* task.
    let sessions = crate::worker::pty::PtyManager::new();
    let send = send_fn(|_, _| async {});
    let live = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let is_live = {
        let live = live.clone();
        std::sync::Arc::new(move || live.load(std::sync::atomic::Ordering::SeqCst))
    };

    let handle = spawn_session_stream(
        sessions.clone(),
        spec("t1", "w_1", "peer", 10, is_live),
        send,
    );

    live.store(false, std::sync::atomic::Ordering::SeqCst);
    tokio::time::timeout(std::time::Duration::from_secs(5), handle)
        .await
        .expect("the stream ends once its task is over")
        .expect("and ends cleanly rather than panicking");
}

#[tokio::test]
async fn the_router_ignores_messages_it_is_not_the_receiver_for() {
    // An ack is accepted and does nothing; a frame arriving at the sender is a
    // peer with the protocol backwards. Neither may panic or start a stream.
    let mut router = empty_router();
    router.handle(
        "peerA",
        medulla::protocol::ScreenMessage::Ack {
            task_id: "t1".into(),
            seq: 4,
        },
    );
    router.handle(
        "peerA",
        medulla::protocol::ScreenMessage::Frame(medulla::protocol::ScreenFrame {
            task_id: "t1".into(),
            seq: 1,
            base_seq: 0,
            full: true,
            cols: 1,
            rows: 1,
            cursor: (0, 0),
            hide_cursor: false,
            rows_changed: Vec::new(),
        }),
    );
    assert_eq!(router.active(), 0);
}

#[tokio::test]
async fn a_dispatch_watcher_may_not_type_into_the_session_it_watches() {
    // The authorization boundary between the two kinds of viewer. This router's
    // subscribers are machines that dispatched work, and the task-ownership
    // check that lets them *watch* was never meant to also let them drive. An
    // operator attached to their own remote session is served by the host-serve
    // relay instead, which knows who is attached.
    let mut router = empty_router();
    router.handle(
        "peerA",
        medulla::protocol::ScreenMessage::input("t1", b"rm -rf /\r"),
    );
    router.handle(
        "peerA",
        medulla::protocol::ScreenMessage::Resize {
            task_id: "t1".into(),
            cols: 80,
            rows: 24,
        },
    );
    assert_eq!(
        router.active(),
        0,
        "neither message may start or touch a stream"
    );
}
