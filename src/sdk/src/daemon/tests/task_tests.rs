//! Task-lifecycle tests: acceptance limits, duplicate rejection, stdin/input
//! forwarding (including buffering before the sink registers), and shutdown
//! aborting an in-flight task.
//!
//! Sender attribution and its logging live in the sibling
//! `task_attribution_tests`; conversation continuity (resume, isolation, and
//! the turn-serialization race) lives in `task_continuity_tests` — both split
//! out once their groups pushed this file over the 500-line ceiling.

use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::{mpsc, Notify};

use crate::daemon::providers::{RunTaskFn, RunTaskOptions, RunTaskResult};
use crate::daemon::DaemonRuntime;
use crate::tinyplace::TaskFrameKind;

use super::{
    abort_frame, abortable_runner, base_config, blocking_runner, decoded_frames, input_frame,
    recording_send, stdin_runner, task_frame, wait_ready,
};

#[tokio::test]
async fn rejects_tasks_over_max_pending() {
    let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Notify::new());
    let run_task = blocking_runner(ready_tx, gate.clone());
    let (send, recorded) = recording_send();
    let mut config = base_config();
    config.concurrency = 1;
    config.max_pending = 1;
    let runtime = DaemonRuntime::new(config, run_task, send);

    // Task A occupies the single pending slot and blocks.
    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t1", "work", None)),
    );
    wait_ready(&mut ready_rx).await;

    // Task B is rejected at capacity.
    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t2", "more", None)),
    );
    // Let B settle (it errors without ever running).
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let frames = decoded_frames(&recorded);
    let capacity = frames
        .iter()
        .find(|f| f.kind == TaskFrameKind::Error && f.task_id == "t2")
        .expect("t2 should be rejected");
    assert!(
        capacity.text.contains("at capacity"),
        "got: {}",
        capacity.text
    );

    gate.notify_waiters();
    runtime.idle().await;
}

#[tokio::test]
async fn a_panicking_run_releases_its_admission_slot() {
    // The task handler runs under a bare `tokio::spawn` that is never joined, so
    // a panic inside it is swallowed: no frame reaches the peer and nothing
    // reports the failure. What must NOT also happen is the admission count
    // staying up — a hand-rolled decrement on the straight-line path leaks one
    // slot per unwind, and `max_pending` unwinds over a worker's lifetime pin it
    // at "daemon at capacity" until it is restarted. The count is released by a
    // guard, so the very next task is still admitted.
    let run_task: RunTaskFn = Arc::new(|_opts: RunTaskOptions| {
        Box::pin(async move { panic!("harness executor exploded") })
    });
    let (send, recorded) = recording_send();
    let mut config = base_config();
    config.max_pending = 1;
    let runtime = DaemonRuntime::new(config, run_task, send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t1", "work", None)),
    );
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // A second task, after the first one's slot should have been released.
    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t2", "more", None)),
    );
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let frames = decoded_frames(&recorded);
    assert!(
        frames
            .iter()
            .any(|f| f.kind == TaskFrameKind::Ack && f.task_id == "t2"),
        "the unwound task must not have kept its admission slot: {frames:?}"
    );
    assert!(
        !frames
            .iter()
            .any(|f| f.kind == TaskFrameKind::Error && f.text.contains("at capacity")),
        "no capacity rejection should have been sent: {frames:?}"
    );
    // ...and the task id it stranded is free again, rather than refusing every
    // later dispatch that reuses it.
    assert!(
        runtime.session_for_task("peer", "t1").is_none(),
        "the running record must be released with the admission"
    );
}

#[tokio::test]
async fn a_task_waiting_for_a_slot_reports_that_it_is_queued() {
    // The wait for a harness slot is otherwise completely silent: the task acks
    // immediately, then sends nothing at all until a slot frees. With 16 pending
    // tasks allowed behind 2 slots that silence routinely outlasts the
    // requester's no-progress watchdog, which reaps the dispatch as
    // `bridge task timed out` while the task is perfectly healthy and simply
    // queued. A heartbeat while queued is what makes the two distinguishable.
    let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Notify::new());
    let run_task = blocking_runner(ready_tx, gate.clone());
    let (send, recorded) = recording_send();
    let mut config = base_config();
    config.concurrency = 1;
    // Heartbeat = 8 throttle windows, so 20ms here is a 160ms heartbeat.
    config.status_throttle_ms = 20;
    let runtime = DaemonRuntime::new(config, run_task, send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t1", "work", None)),
    );
    wait_ready(&mut ready_rx).await;

    // t2 is admitted but has nowhere to run: the only slot is taken.
    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t2", "more", None)),
    );
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    let frames = decoded_frames(&recorded);
    let queued = frames
        .iter()
        .filter(|f| {
            f.kind == TaskFrameKind::Status
                && f.task_id == "t2"
                && f.text == "queued for a harness slot"
        })
        .count();
    assert!(
        queued >= 2,
        "a queued task must keep reporting itself alive: {frames:?}"
    );

    // Deliberately no `idle()`: releasing the gate lets t1 finish and t2 take
    // the freed slot, where it would block on a gate that has already fired.
    // The runtime is dropped with the test instead.
    runtime.shutdown();
}

#[tokio::test]
async fn an_abort_while_queued_stops_the_task_before_it_runs() {
    // The queue wait used to be a bare `.await` on the semaphore, so an abort
    // arriving while a task was queued did nothing: the task acquired its slot
    // later and ran work whose requester had long since given up — holding a
    // harness AND holding its task id against the duplicate guard.
    let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Notify::new());
    let run_task = blocking_runner(ready_tx, gate.clone());
    let (send, recorded) = recording_send();
    let mut config = base_config();
    config.concurrency = 1;
    let runtime = DaemonRuntime::new(config, run_task, send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t1", "work", Some("corr-A"))),
    );
    wait_ready(&mut ready_rx).await;

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t2", "more", Some("corr-B"))),
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(abort_frame("t2", Some("corr-B"))),
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let frames = decoded_frames(&recorded);
    assert!(
        frames.iter().any(|f| f.kind == TaskFrameKind::Error
            && f.task_id == "t2"
            && f.text.contains("aborted while queued")),
        "the queued task must report that it was stopped: {frames:?}"
    );

    // Freeing the first slot must not resurrect it.
    gate.notify_waiters();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        !decoded_frames(&recorded)
            .iter()
            .any(|f| f.kind == TaskFrameKind::Reply && f.task_id == "t2"),
        "an aborted queued task must never run"
    );

    runtime.shutdown();
}

#[tokio::test]
async fn a_running_task_heartbeats_through_a_silent_stretch() {
    // Status frames are driven entirely by harness events, and one long tool
    // call — a repository-wide search, a slow test run — produces none for
    // minutes at a time. To the requester that is indistinguishable from a
    // crashed worker. The heartbeat is the floor that makes it distinguishable.
    let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Notify::new());
    // Deliberately not `blocking_runner`: an `async move` block captures only
    // the fields it names, so that helper drops the rest of `RunTaskOptions` —
    // including the event callback holding the status channel — the instant it
    // is called, which closes the channel and ends the consumer. A real
    // executor holds its options for the whole run, and so does this one.
    let run_task: RunTaskFn = {
        let gate = gate.clone();
        Arc::new(move |opts: RunTaskOptions| {
            let ready = ready_tx.clone();
            let gate = gate.clone();
            Box::pin(async move {
                let opts = opts;
                let _ = ready.send(());
                gate.notified().await;
                Ok(RunTaskResult {
                    session_id: None,
                    usage: None,
                    provider: opts.provider,
                    reply: "done".to_string(),
                    events: 0,
                })
            })
        })
    };
    let (send, recorded) = recording_send();
    let mut config = base_config();
    config.status_throttle_ms = 20; // → a 160ms heartbeat
    let runtime = DaemonRuntime::new(config, run_task, send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t1", "work", None)),
    );
    wait_ready(&mut ready_rx).await;
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    let beats = decoded_frames(&recorded)
        .iter()
        .filter(|f| {
            f.kind == TaskFrameKind::Status && f.task_id == "t1" && f.text == "still working"
        })
        .count();
    assert!(
        beats >= 2,
        "a running task that emits no events must still report liveness (got {beats})"
    );

    gate.notify_waiters();
    runtime.idle().await;
}

#[tokio::test]
async fn rejects_duplicate_task_id_from_same_sender() {
    let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Notify::new());
    let run_task = blocking_runner(ready_tx, gate.clone());
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), run_task, send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("dup", "one", None)),
    );
    wait_ready(&mut ready_rx).await;

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("dup", "two", None)),
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let frames = decoded_frames(&recorded);
    let dup_error = frames
        .iter()
        .find(|f| f.kind == TaskFrameKind::Error && f.task_id == "dup")
        .expect("duplicate should error");
    assert!(
        dup_error.text.contains("already running"),
        "got: {}",
        dup_error.text
    );
    // The original ack is still present.
    assert!(frames
        .iter()
        .any(|f| f.kind == TaskFrameKind::Ack && f.text == "task accepted"));

    gate.notify_waiters();
    runtime.idle().await;
}

#[tokio::test]
async fn forwards_input_into_running_task() {
    let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Notify::new());
    let received = Arc::new(StdMutex::new(Vec::new()));
    let run_task = stdin_runner(ready_tx, gate.clone(), received.clone());
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), run_task, send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t1", "work", None)),
    );
    wait_ready(&mut ready_rx).await;

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(input_frame("t1", "extra guidance", None)),
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert_eq!(
        received.lock().unwrap().as_slice(),
        &["extra guidance".to_string()]
    );
    let frames = decoded_frames(&recorded);
    assert!(frames
        .iter()
        .any(|f| f.kind == TaskFrameKind::Ack && f.text == "input received"));

    gate.notify_waiters();
    runtime.idle().await;
}

#[tokio::test]
async fn abort_with_mismatched_correlation_leaves_the_running_task_alone() {
    // Task ids recur by construction — they are positional per `delegate_tasks`
    // call, and the hub's uniquifying suffix restarts from zero when the hub
    // does. So an abort for a task that has already finished can name a live
    // one, and cancelling that is silent and total: the peer waiting on the new
    // task simply never hears back.
    //
    // `handle_input` has guarded this since it was written; `handle_abort` did
    // not, and it is the more damaging of the two.
    let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Notify::new());
    let received = Arc::new(StdMutex::new(Vec::new()));
    let run_task = stdin_runner(ready_tx, gate.clone(), received.clone());
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), run_task, send);

    // The live task, from a *later* dispatch that reused the id.
    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t1", "work", Some("corr-B"))),
    );
    wait_ready(&mut ready_rx).await;
    assert_eq!(runtime.active_count(), 1);

    // A stale abort for the earlier dispatch of the same id.
    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(abort_frame("t1", Some("corr-A"))),
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert_eq!(
        runtime.active_count(),
        1,
        "a stale abort must not cancel the task that reused the id"
    );
    let frames = decoded_frames(&recorded);
    assert!(
        frames
            .iter()
            .any(|f| f.kind == TaskFrameKind::Ack && f.text == "no matching running task to abort"),
        "acked either way, but as a no-match: {frames:?}"
    );

    gate.notify_waiters();
    runtime.shutdown();
    runtime.idle().await;
}

#[tokio::test]
async fn an_abort_that_matches_the_running_dispatch_stops_it() {
    // The other half: the guard must not make Abort inert.
    let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Notify::new());
    let received = Arc::new(StdMutex::new(Vec::new()));
    let run_task = stdin_runner(ready_tx, gate.clone(), received.clone());
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), run_task, send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t1", "work", Some("corr-A"))),
    );
    wait_ready(&mut ready_rx).await;

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(abort_frame("t1", Some("corr-A"))),
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let frames = decoded_frames(&recorded);
    assert!(
        frames
            .iter()
            .any(|f| f.kind == TaskFrameKind::Ack && f.text == "task aborted"),
        "a matching abort must still stop the task: {frames:?}"
    );

    gate.notify_waiters();
    runtime.shutdown();
    runtime.idle().await;
}

#[tokio::test]
async fn input_with_mismatched_correlation_does_not_match() {
    let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Notify::new());
    let received = Arc::new(StdMutex::new(Vec::new()));
    let run_task = stdin_runner(ready_tx, gate.clone(), received.clone());
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), run_task, send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t1", "work", Some("corr-A"))),
    );
    wait_ready(&mut ready_rx).await;

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(input_frame("t1", "wrong dispatch", Some("corr-B"))),
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    assert!(
        received.lock().unwrap().is_empty(),
        "mismatched correlation must not forward"
    );
    let frames = decoded_frames(&recorded);
    assert!(frames
        .iter()
        .any(|f| f.kind == TaskFrameKind::Ack && f.text == "no matching running task for input"));

    gate.notify_waiters();
    runtime.idle().await;
}

#[tokio::test]
async fn shutdown_aborts_in_flight_task() {
    let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
    let run_task = abortable_runner(ready_tx);
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), run_task, send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t1", "work", None)),
    );
    wait_ready(&mut ready_rx).await;
    assert_eq!(runtime.active_count(), 1, "one task in flight");

    runtime.shutdown();
    runtime.idle().await;

    let frames = decoded_frames(&recorded);
    assert!(frames
        .iter()
        .any(|f| f.kind == TaskFrameKind::Error && f.text.contains("aborted")));
    assert_eq!(runtime.active_count(), 0, "no tasks after shutdown");
}

#[tokio::test]
async fn input_for_unknown_task_is_not_matched() {
    let run_task: RunTaskFn = Arc::new(|opts: RunTaskOptions| {
        Box::pin(async move {
            Ok(RunTaskResult {
                session_id: None,
                usage: None,
                provider: opts.provider,
                reply: "ok".to_string(),
                events: 0,
            })
        })
    });
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), run_task, send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(input_frame("ghost", "hi", None)),
    );
    runtime.idle().await;

    let frames = decoded_frames(&recorded);
    assert!(frames
        .iter()
        .any(|f| f.kind == TaskFrameKind::Ack && f.text == "no matching running task for input"));
}

#[tokio::test]
async fn input_buffered_before_stdin_registration_is_drained() {
    // A runner that starts (ready) but registers its stdin sink only after a gate
    // is released — so an `input` arriving in between must buffer in pending_input
    // and flush when the sink registers.
    let ready = Arc::new(Notify::new());
    let gate = Arc::new(Notify::new());
    let received = Arc::new(StdMutex::new(Vec::new()));
    let run_task: RunTaskFn = {
        let ready = ready.clone();
        let gate = gate.clone();
        let received = received.clone();
        Arc::new(move |mut opts: RunTaskOptions| {
            let ready = ready.clone();
            let gate = gate.clone();
            let received = received.clone();
            Box::pin(async move {
                ready.notify_waiters();
                gate.notified().await; // hold off registration until released
                let (tx, mut rx) = mpsc::unbounded_channel::<String>();
                if let Some(register) = opts.on_stdin.take() {
                    register(tx); // drains any buffered pending_input into tx
                }
                let sink = received.clone();
                let reader = tokio::spawn(async move {
                    while let Some(line) = rx.recv().await {
                        sink.lock().unwrap().push(line);
                    }
                });
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                reader.abort();
                Ok(RunTaskResult {
                    session_id: None,
                    usage: None,
                    provider: opts.provider,
                    reply: "done".to_string(),
                    events: 0,
                })
            })
        })
    };
    let (send, _recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), run_task, send);

    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(task_frame("t1", "work", None)),
    );
    ready.notified().await;
    // Input arrives before the stdin sink exists → buffered as pending_input.
    runtime.handle_message(
        "peer".into(),
        String::new(),
        Some(input_frame("t1", "buffered guidance", None)),
    );
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    gate.notify_waiters(); // now registration drains the buffer
    runtime.idle().await;

    assert_eq!(
        received.lock().unwrap().as_slice(),
        &["buffered guidance".to_string()]
    );
}

#[tokio::test]
async fn a_screen_message_is_never_typed_into_a_harness() {
    // The plain-text path runs whatever it is handed as a prompt. A daemon with
    // no watchable sessions cannot serve `medulla.screen.v1`, but "cannot serve"
    // must mean ignored, not executed — otherwise watching a task on a
    // device-local or headless daemon dispatches the subscribe JSON to claude,
    // which is what it did.
    let (ready_tx, mut ready_rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Notify::new());
    let run_task = blocking_runner(ready_tx, gate.clone());
    let (send, recorded) = recording_send();
    let runtime = DaemonRuntime::new(base_config(), run_task, send);

    for body in [
        crate::tinyplace::encode_screen_message(&crate::tinyplace::ScreenMessage::Subscribe {
            task_id: "t1".into(),
            max_fps: 1,
            resync: true,
        }),
        crate::tinyplace::encode_screen_message(&crate::tinyplace::ScreenMessage::Unsubscribe {
            task_id: "t1".into(),
        }),
    ] {
        runtime.handle_message("peer".into(), body, None);
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Nothing reached a harness...
    assert!(
        ready_rx.try_recv().is_err(),
        "a screen message must not start a run"
    );
    // ...and nothing was sent back, since the hub is owed no answer to a message
    // this daemon has no way to serve.
    assert!(recorded.lock().unwrap().is_empty());
}
