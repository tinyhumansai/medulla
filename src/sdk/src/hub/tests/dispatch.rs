//! Tests for the sender-runner: dispatch, routing by `correlationId`, the ack
//! window + reset/resend recovery, the no-progress watchdog, and orchestrator
//! abort. Driven by the [`FakeWorker`](harness::FakeWorker) [`Relay`] harness,
//! which replays a worker's frame sequence into the inbox with no network.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use super::super::types::RunError;
use super::super::ActivityLog;
use super::super::TaskRunner;

mod harness;
use harness::{req, FakeWorker, Mode};

#[tokio::test]
async fn dispatches_and_returns_the_worker_reply_with_usage() {
    let worker = FakeWorker::new(Mode::Reply("REMOTE: 4 agents, 1 offline".to_string()));
    let runner = TaskRunner::start(worker, Duration::from_millis(5));

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let outcome = runner.run(req("audit"), Some(tx)).await.expect("ok");

    assert_eq!(outcome.reply, "REMOTE: 4 agents, 1 offline");
    assert_eq!(outcome.usage.input_tokens, 3);
    assert_eq!(outcome.usage.output_tokens, 5);
    // The status frame was forwarded to the progress sink.
    assert_eq!(rx.recv().await.as_deref(), Some("running python audit.py"));
}

#[tokio::test]
async fn surfaces_a_worker_error_frame() {
    let worker = FakeWorker::new(Mode::Error("boom".to_string()));
    let runner = TaskRunner::start(worker, Duration::from_millis(5));

    let err = runner.run(req("x"), None).await.expect_err("errors");
    assert_eq!(err, RunError::Worker("boom".to_string()));
}

#[tokio::test]
async fn times_out_when_the_worker_never_replies() {
    // A permanently-silent peer: the runner resets + resends up to the cap, then
    // gives up with Timeout. Short ack window so the whole retry loop is quick.
    let worker = FakeWorker::new(Mode::Silent);
    let runner = TaskRunner::start_with_ack_window(
        worker,
        Duration::from_millis(5),
        Duration::from_millis(40),
    );
    let err = runner.run(req("x"), None).await.expect_err("times out");
    assert_eq!(err, RunError::Timeout);
}

#[tokio::test]
async fn recovers_from_a_desynced_peer_by_resetting_and_resending() {
    // The peer is silent on the first send (its post-restart CIPHERTEXT would be
    // undecryptable), then answers once the session has been reset. The runner
    // should reset + resend within the ack window and succeed.
    let worker = FakeWorker::new(Mode::RecoverAfterReset("recovered".to_string()));
    let runner = TaskRunner::start_with_ack_window(
        worker.clone(),
        Duration::from_millis(5),
        Duration::from_millis(120),
    );
    let outcome = runner.run(req("x"), None).await.expect("recovers");
    assert_eq!(outcome.reply, "recovered");
    assert!(
        worker.resets.load(Ordering::Relaxed) >= 1,
        "the runner should have reset the session before resending"
    );
}

#[tokio::test]
async fn concurrent_dispatches_are_routed_by_correlation_id() {
    // Two tasks in flight at once must each get their own reply, proving the
    // shared pump fans frames out by correlationId rather than crossing wires.
    let worker = FakeWorker::new(Mode::Reply("done".to_string()));
    let runner = Arc::new(TaskRunner::start(worker, Duration::from_millis(5)));

    let a = {
        let r = runner.clone();
        let mut req_a = req("a");
        req_a.task_id = "ta".to_string();
        req_a.abort_id = "ta".to_string();
        tokio::spawn(async move { r.run(req_a, None).await })
    };
    let b = {
        let r = runner.clone();
        let mut req_b = req("b");
        req_b.task_id = "tb".to_string();
        req_b.abort_id = "tb".to_string();
        tokio::spawn(async move { r.run(req_b, None).await })
    };
    assert_eq!(a.await.unwrap().unwrap().reply, "done");
    assert_eq!(b.await.unwrap().unwrap().reply, "done");
}

#[tokio::test]
async fn surfaces_a_transport_error_when_the_send_fails() {
    // The relay's send fails outright (e.g. the address can't be decoded): the
    // runner drops the waiter and returns a Transport error, not a hang.
    let worker = FakeWorker::with(Mode::Reply("unused".to_string()), true, 0);
    let runner = TaskRunner::start(worker, Duration::from_millis(5));

    let err = runner.run(req("x"), None).await.expect_err("send fails");
    assert_eq!(err, RunError::Transport("send boom".to_string()));
}

#[tokio::test]
async fn waits_for_contact_acceptance_before_dispatching() {
    // The peer isn't a contact yet: the runner requests one and polls until the
    // auto-accepter settles (here, on the third check) before it sends the task.
    let worker = FakeWorker::with(Mode::Reply("hi".to_string()), false, 2);
    let runner = TaskRunner::start(worker.clone(), Duration::from_millis(5));

    let outcome = runner
        .run(req("x"), None)
        .await
        .expect("dispatches once accepted");
    assert_eq!(outcome.reply, "hi");
    assert!(
        worker.contact_checks.load(Ordering::Relaxed) >= 2,
        "the runner should have polled contact status until acceptance"
    );
}

#[tokio::test]
async fn reaps_a_peer_that_acks_but_then_goes_silent() {
    // The peer answers (ack + status) — so the runner commits past the ack window
    // rather than resetting — but never sends a terminal frame. The no-progress
    // (idle) watchdog must still fire: a worker that acks and then dies would
    // otherwise pin its correlation entry and spawned handler forever, since the
    // terminal frame that settles the waiter is never coming.
    let worker = FakeWorker::new(Mode::AckOnly);
    // Short idle window so the watchdog fires quickly; a live worker's frames
    // would reset it, but AckOnly sends none after the first burst.
    let runner = TaskRunner::start_with_idle_window(
        worker,
        Duration::from_millis(5),
        Duration::from_millis(80),
    );

    let err = runner
        .run(req("x"), None)
        .await
        .expect_err("times out after ack");
    assert_eq!(err, RunError::Timeout);
}

#[test]
fn run_error_display_is_human_readable_per_variant() {
    assert_eq!(RunError::Timeout.to_string(), "tiny.place task timed out");
    assert_eq!(
        RunError::Aborted.to_string(),
        "task aborted by orchestrator"
    );
    assert_eq!(
        RunError::Worker("boom".to_string()).to_string(),
        "worker error: boom"
    );
    assert_eq!(
        RunError::Transport("no route".to_string()).to_string(),
        "transport error: no route"
    );
}

// ------------------------------------------------------- contact on add ---

/// Adding a peer opens the contact edge; re-adding retries it.
///
/// Deferring the request to first dispatch — which is what used to happen —
/// makes adding a worker look like nothing happened: the peer's operator sees no
/// approval to give, and when one finally appears it is attached to a task
/// already blocked on it. Re-adding is then the natural way to retry a request
/// the peer missed.
///
/// The decision is tested here; the socket plumbing around it belongs to the
/// live staging E2E, as the rest of `HubHandle` does.

#[tokio::test]
async fn every_inbound_worker_frame_is_narrated_with_its_payload() {
    // The hub reported only the settled outcome, so "the worker never answered"
    // and "the worker answered with nothing" read identically from the
    // orchestrator's side — and neither said whether the worker had been
    // talking at all. Every frame it sends is now on the record.
    let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let worker = FakeWorker::new(Mode::Reply("REMOTE: 4 agents, 1 offline".to_string()));
    let runner = TaskRunner::start_with_log(worker, Duration::from_millis(5), {
        let seen = seen.clone();
        Arc::new(move |line: &str| seen.lock().unwrap().push(line.to_string()))
    });

    let outcome = runner.run(req("audit"), None).await.expect("ok");
    assert_eq!(outcome.reply, "REMOTE: 4 agents, 1 offline");

    let lines = seen.lock().unwrap().clone();
    let reply_line = lines
        .iter()
        .find(|l| l.contains("reply"))
        .unwrap_or_else(|| panic!("the reply must be narrated, got {lines:?}"));
    assert!(
        reply_line.contains("REMOTE: 4 agents, 1 offline"),
        "the payload itself is the point: {reply_line}"
    );
    assert!(
        reply_line.contains("27 chars"),
        "a truncated preview cannot report its own truncation: {reply_line}"
    );
    // The status frame that preceded it is on the record too, so a worker that
    // is working but not finishing is distinguishable from a silent one.
    assert!(
        lines.iter().any(|l| l.contains("running python audit.py")),
        "got {lines:?}"
    );
}

#[test]
fn a_preview_is_clipped_and_flattened_but_says_so() {
    use crate::logging::{preview, PREVIEW_CHARS};

    assert_eq!(preview("one\ntwo   three"), "one two three");
    let long = "x".repeat(PREVIEW_CHARS + 50);
    let out = preview(&long);
    assert!(out.ends_with('…'), "truncation must be visible: {out}");
    assert_eq!(out.chars().count(), PREVIEW_CHARS + 1);
    assert_eq!(preview(""), "");
}

#[tokio::test]
async fn a_worker_that_keeps_reporting_progress_is_not_timed_out() {
    // The bound is IDLE, not wall-clock: every frame resets it, so a worker
    // streaming `running Bash: …` is left to work however long it runs. There is
    // NO hard ceiling — the hub owns no task deadline (the backend does). The
    // streamed frames here span ~800ms (20 statuses at a 40ms poll), far longer
    // than the 200ms idle window and than any single budget, and the reply still
    // lands — a wall-clock cap would have killed it long before. Every frame
    // resets the clock, so only the gaps BETWEEN frames (~40ms) are measured
    // against the window, never the total.
    let worker = FakeWorker::new(Mode::Chatty {
        statuses: 20,
        reply: "scan complete".to_string(),
    });
    let runner = TaskRunner::start_with_idle_window(
        worker,
        Duration::from_millis(40),
        Duration::from_millis(200),
    );

    let outcome = runner
        .run(req("scan the filesystem"), None)
        .await
        .expect("must not time out");
    assert_eq!(outcome.reply, "scan complete");
}

#[test]
fn each_dispatch_gets_its_own_worker_facing_task_id() {
    // `delegate_tasks` names unnamed tasks positionally per call, so every call
    // starts again at `t1`. The worker dedupes on sender + taskId, so without a
    // per-dispatch id the second call's `t1` is refused as a duplicate of the
    // first's — three dispatches named `t1`, carrying three different
    // instructions, two refused. Observed in the field.
    let a = super::super::socket::wire_task_id("t1");
    let b = super::super::socket::wire_task_id("t1");
    assert_ne!(
        a, b,
        "two dispatches of `t1` must not collide on the worker"
    );
    // The original id stays legible in it, so a worker log line can still be
    // traced back to the task the orchestrator named.
    assert!(a.starts_with("t1#"), "got {a}");
    assert!(b.starts_with("t1#"), "got {b}");
    // A named task keeps its name too.
    assert!(super::super::socket::wire_task_id("repo-scan").starts_with("repo-scan#"));
}

#[tokio::test]
async fn giving_up_tells_the_worker_to_stop() {
    // An abandoned task keeps a harness busy AND keeps its id live — a responder
    // refuses a later task whose id is already running, and unnamed tasks are
    // named positionally, so that id is very often `t1`. Without this, one task
    // the hub gave up on poisons `t1` for the rest of the responder's own
    // timeout. So when the no-progress watchdog reaps a silent-after-ack peer, it
    // must tell the worker to stop, not just drop the waiter.
    let worker = FakeWorker::new(Mode::AckOnly);
    let runner = TaskRunner::start_with_idle_window(
        worker.clone(),
        Duration::from_millis(1),
        Duration::from_millis(20),
    );

    let err = runner
        .run(req("scan the filesystem"), None)
        .await
        .expect_err("must time out");
    assert!(matches!(err, RunError::Timeout), "got {err:?}");

    let aborts = worker.sent_kinds().await;
    assert!(
        aborts.iter().any(|k| k == "abort"),
        "an abandoned task must be cancelled, got {aborts:?}"
    );
}

#[tokio::test]
async fn an_orchestrator_abort_stops_the_worker_and_reaps() {
    // The backend owns the deadline and cancels a running task via
    // `medulla:task_abort`. A healthy worker — here acked and working, so no
    // liveness bound would ever fire — must still stop when the orchestrator
    // aborts. The default idle window is minutes away, so ONLY the abort can end
    // this run; if it hangs, the abort path is broken.
    let worker = FakeWorker::new(Mode::AckOnly);
    let runner = Arc::new(TaskRunner::start(worker.clone(), Duration::from_millis(2)));

    let r = runner.clone();
    let handle = tokio::spawn(async move { r.run(req("x"), None).await });

    // Abort by the orchestrator-facing id (`req`'s `abort_id` is "t1"). Retried
    // until the run resolves: a notify sent before `run` has registered its
    // signal is a no-op, so a single early call could miss the dispatch.
    loop {
        runner.abort_task("t1");
        if handle.is_finished() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let err = handle.await.unwrap().expect_err("aborted");
    assert_eq!(err, RunError::Aborted);

    // The worker was told to stop, not just dropped — otherwise the task keeps a
    // harness busy and poisons its id for the responder's own (far longer) timeout.
    let kinds = worker.sent_kinds().await;
    assert!(
        kinds.iter().any(|k| k == "abort"),
        "an aborted task must be cancelled on the worker, got {kinds:?}"
    );
}

#[tokio::test]
async fn an_abort_before_the_worker_acks_stops_it_too() {
    // A dispatched-but-not-yet-acked worker: the abort is caught in the PRE-ack
    // select (before any sign of life), which cancels the worker and returns
    // Aborted — distinct from the post-ack path exercised above.
    let worker = FakeWorker::new(Mode::Silent);
    let runner = Arc::new(TaskRunner::start(worker.clone(), Duration::from_millis(2)));

    let r = runner.clone();
    let handle = tokio::spawn(async move { r.run(req("x"), None).await });

    loop {
        runner.abort_task("t1");
        if handle.is_finished() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let err = handle.await.unwrap().expect_err("aborted");
    assert_eq!(err, RunError::Aborted);
    // The task was dispatched, then the abort frame followed it.
    let kinds = worker.sent_kinds().await;
    assert!(kinds.iter().any(|k| k == "task"), "got {kinds:?}");
    assert!(kinds.iter().any(|k| k == "abort"), "got {kinds:?}");
}

#[tokio::test]
async fn records_activity_when_built_with_an_activity_log() {
    // `start_with_log_and_activity` wires an ActivityLog the pump feeds as frames
    // arrive, so the Agents view can render what each worker is doing. Every
    // inbound frame (ack, status, reply) is recorded.
    let activity = ActivityLog::new();
    let worker = FakeWorker::new(Mode::Reply("scan done".to_string()));
    let runner = TaskRunner::start_with_log_and_activity(
        worker,
        Duration::from_millis(5),
        Arc::new(|_l: &str| {}),
        activity.clone(),
    );

    let outcome = runner.run(req("audit"), None).await.expect("ok");
    assert_eq!(outcome.reply, "scan done");
    assert!(
        !activity.snapshot().is_empty(),
        "the activity log should have recorded the worker's frames"
    );
}

#[tokio::test]
async fn the_pump_skips_an_undecodable_frame_and_keeps_going() {
    // A message the pump can't decode (a stray DM or corrupt payload in the shared
    // inbox) must be skipped, not fatal: the real reply that follows still settles.
    let worker = FakeWorker::new(Mode::GarbageThenReply("recovered".to_string()));
    let runner = TaskRunner::start(worker, Duration::from_millis(5));
    let outcome = runner
        .run(req("x"), None)
        .await
        .expect("ok despite garbage");
    assert_eq!(outcome.reply, "recovered");
}

#[tokio::test]
async fn an_abort_during_contact_negotiation_is_honored() {
    // A first-time worker isn't a contact yet, so `run` polls for acceptance
    // (up to CONTACT_WAIT) before dispatching. An abort that arrives in that
    // window must still be honored — the abort signal is registered BEFORE the
    // wait, so it is not silently dropped — and it must bail before any task
    // frame reaches the worker. `accept_after` is set high so acceptance never
    // settles on its own; only the abort can end this run.
    let worker = FakeWorker::with(Mode::Reply("unused".to_string()), false, 10_000);
    let runner = Arc::new(TaskRunner::start(worker.clone(), Duration::from_millis(2)));

    let r = runner.clone();
    let handle = tokio::spawn(async move { r.run(req("x"), None).await });

    loop {
        runner.abort_task("t1");
        if handle.is_finished() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let err = handle.await.unwrap().expect_err("aborted");
    assert_eq!(err, RunError::Aborted);
    // Nothing was dispatched — the abort bailed before the task frame was sent.
    assert!(
        worker.sent_kinds().await.is_empty(),
        "no task should reach a worker aborted during contact negotiation"
    );
}

#[tokio::test]
async fn aborting_an_unknown_task_is_a_harmless_no_op() {
    // A `task_abort` for a task that already settled (or was never dispatched
    // here) must not panic or block — the registry simply has no entry.
    let worker = FakeWorker::new(Mode::Reply("done".to_string()));
    let runner = TaskRunner::start(worker, Duration::from_millis(5));
    runner.abort_task("never-dispatched");
    // The runner is still fully usable afterwards.
    let outcome = runner.run(req("x"), None).await.expect("ok");
    assert_eq!(outcome.reply, "done");
}
