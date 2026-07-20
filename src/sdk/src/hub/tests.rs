//! Unit tests for the sender-runner, driven by a `FakeWorker` [`Relay`] that
//! replays the daemon's `ack → status → reply|error` sequence into the inbox so
//! the runner exercises its full dispatch/route/settle path with no network.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::daemon::transport::InboundMessage;
use crate::tinyplace::{
    decode_task_frame, encode_task_frame_with_usage, EncodeFrameInput, TaskFrameKind, TokenUsage,
};

use super::relay::Relay;
use super::roster::{address_of, register_payload, HubWorker};
use super::types::{RunError, TaskRequest};

fn worker(id: &str, addr: &str) -> HubWorker {
    HubWorker {
        id: id.to_string(),
        address: addr.to_string(),
        harness: "claude".to_string(),
        label: None,
        selected: false,
    }
}

#[test]
fn register_payload_advertises_id_address_and_harness() {
    let payload = register_payload(&[worker("w1", "GRVaddr")]);
    let agents = payload.get("agents").unwrap().as_array().unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0]["id"], "w1");
    assert_eq!(agents[0]["metadata"]["address"], "GRVaddr");
    assert_eq!(agents[0]["metadata"]["harness"], "claude");
}

#[test]
fn address_of_resolves_known_agent_and_falls_back_to_first_worker() {
    let workers = [worker("w1", "ADDR1"), worker("w2", "ADDR2")];
    assert_eq!(address_of(&workers, "w2").as_deref(), Some("ADDR2"));
    // Unknown / empty agentId (backend omitted it): fall back to the first
    // worker, NOT the empty id (which would decode to a zero-length key).
    assert_eq!(address_of(&workers, "").as_deref(), Some("ADDR1"));
    assert_eq!(address_of(&workers, "unknown").as_deref(), Some("ADDR1"));
    assert_eq!(address_of(&[], "w1"), None);
}
use super::TaskRunner;

/// How the fake worker responds to a dispatched task.
enum Mode {
    Reply(String),
    Error(String),
    Silent,
    /// Silent until the sender has reset the session (simulating a restarted peer
    /// whose first `CIPHERTEXT` is undecryptable), then replies.
    RecoverAfterReset(String),
}

/// A fake worker: on `send`, decodes the task frame and queues the daemon's
/// `ack → status → (reply|error)` sequence (echoing `correlationId`), which the
/// pump then drains. `Silent` queues nothing, to exercise the timeout path.
struct FakeWorker {
    inbox: Mutex<VecDeque<InboundMessage>>,
    mode: Mode,
    /// How many times the sender has reset the session with us.
    resets: AtomicU32,
}

impl FakeWorker {
    fn new(mode: Mode) -> Arc<Self> {
        Arc::new(Self {
            inbox: Mutex::new(VecDeque::new()),
            mode,
            resets: AtomicU32::new(0),
        })
    }
}

#[async_trait]
impl Relay for FakeWorker {
    async fn send(&self, _to: &str, body: &str) -> Result<(), String> {
        let frame = decode_task_frame(body).expect("runner sends a valid task frame");
        assert_eq!(frame.kind, TaskFrameKind::Task);
        // Stay silent while there's nothing to say: unconditionally for `Silent`,
        // and until a reset has happened for `RecoverAfterReset`.
        let silent = matches!(self.mode, Mode::Silent)
            || matches!(self.mode, Mode::RecoverAfterReset(_))
                && self.resets.load(Ordering::Relaxed) == 0;
        if silent {
            return Ok(());
        }
        let cid = frame.correlation_id.clone();
        let task_id = frame.task_id.clone();
        let mk = |kind, text: &str, usage| InboundMessage {
            from: "worker".to_string(),
            text: encode_task_frame_with_usage(
                EncodeFrameInput {
                    kind,
                    task_id: task_id.clone(),
                    text: text.to_string(),
                    ts: "T".to_string(),
                    correlation_id: cid.clone(),
                    harness: None,
                    provider: None,
                    model: None,
                },
                usage,
            ),
        };
        let mut q = self.inbox.lock().await;
        q.push_back(mk(TaskFrameKind::Ack, "accepted", None));
        q.push_back(mk(TaskFrameKind::Status, "running python audit.py", None));
        match &self.mode {
            Mode::Reply(text) | Mode::RecoverAfterReset(text) => q.push_back(mk(
                TaskFrameKind::Reply,
                text,
                Some(TokenUsage {
                    input_tokens: 3,
                    output_tokens: 5,
                }),
            )),
            Mode::Error(text) => q.push_back(mk(TaskFrameKind::Error, text, None)),
            Mode::Silent => {}
        }
        Ok(())
    }

    async fn drain_inbox(&self, limit: i64) -> Vec<InboundMessage> {
        let mut q = self.inbox.lock().await;
        let mut out = Vec::new();
        while out.len() < limit as usize {
            match q.pop_front() {
                Some(m) => out.push(m),
                None => break,
            }
        }
        out
    }

    async fn request_contact(&self, _peer: &str) -> Result<(), String> {
        Ok(())
    }

    /// Already a contact, so `run` proceeds straight to the send.
    async fn contact_accepted(&self, _peer: &str) -> bool {
        true
    }

    async fn reset_session(&self, _peer: &str) {
        self.resets.fetch_add(1, Ordering::Relaxed);
    }
}

fn req(instruction: &str) -> TaskRequest {
    TaskRequest {
        task_id: "t1".to_string(),
        cycle_id: Some("c1".to_string()),
        instruction: instruction.to_string(),
        worker_address: "GRV1worker".to_string(),
        provider: None,
        model: None,
        timeout: Duration::from_secs(2),
    }
}

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
        tokio::spawn(async move { r.run(req_a, None).await })
    };
    let b = {
        let r = runner.clone();
        let mut req_b = req("b");
        req_b.task_id = "tb".to_string();
        tokio::spawn(async move { r.run(req_b, None).await })
    };
    assert_eq!(a.await.unwrap().unwrap().reply, "done");
    assert_eq!(b.await.unwrap().unwrap().reply, "done");
}
