//! The `FakeWorker` [`Relay`] the dispatch tests run against.
//!
//! Replays the daemon's `ack → status → reply|error` sequence into the inbox so
//! the runner exercises its full dispatch/route/settle path with no network, plus
//! knobs for the failure modes each test needs (silence, desync-then-recover,
//! send failure, delayed contact acceptance, chatty-but-slow progress).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::daemon::transport::InboundMessage;
use crate::hub::{Relay, TaskRequest};
use crate::tinyplace::{
    decode_task_frame, encode_task_frame_with_usage, EncodeFrameInput, TaskFrameKind, TokenUsage,
};

/// How the fake worker responds to a dispatched task.
pub(super) enum Mode {
    Reply(String),
    Error(String),
    Silent,
    /// Acks (a sign of life) and streams a status, but never sends a terminal
    /// frame — exercises the "peer alive, then goes silent" path the no-progress
    /// watchdog must reap.
    AckOnly,
    /// Silent until the sender has reset the session (simulating a restarted peer
    /// whose first `CIPHERTEXT` is undecryptable), then replies.
    RecoverAfterReset(String),
    /// Streams `statuses` progress frames, one per drain, then replies. Models a
    /// worker that is plainly working but runs longer than the idle window — the
    /// case a wall-clock deadline (or the old ceiling) kills and a frame-resetting
    /// idle watchdog does not.
    Chatty {
        statuses: u32,
        reply: String,
    },
    /// Queues an undecodable message ahead of the normal ack→reply, exercising the
    /// pump's skip-and-continue path for a frame it cannot parse (a stray DM or a
    /// corrupt payload landing in the shared inbox).
    GarbageThenReply(String),
}

pub(super) struct FakeWorker {
    /// The kind of every frame the runner sent us, in order.
    sent: Mutex<Vec<String>>,
    inbox: Mutex<VecDeque<InboundMessage>>,
    mode: Mode,
    /// How many times the sender has reset the session with us.
    pub(super) resets: AtomicU32,
    /// When true, every `send` fails — exercises the transport-error path.
    fail_send: bool,
    /// `contact_accepted` returns false until it has been polled this many times,
    /// simulating a peer whose auto-accepter settles a few polls later.
    accept_after: u32,
    /// How many times `contact_accepted` has been polled.
    pub(super) contact_checks: AtomicU32,
}

impl FakeWorker {
    /// The kinds of frame the runner has sent us, in order.
    pub(super) async fn sent_kinds(&self) -> Vec<String> {
        self.sent.lock().await.clone()
    }

    pub(super) fn new(mode: Mode) -> Arc<Self> {
        Self::with(mode, false, 0)
    }

    /// A worker with explicit send-failure and contact-acceptance-delay knobs.
    pub(super) fn with(mode: Mode, fail_send: bool, accept_after: u32) -> Arc<Self> {
        Arc::new(Self {
            sent: Mutex::new(Vec::new()),
            inbox: Mutex::new(VecDeque::new()),
            mode,
            resets: AtomicU32::new(0),
            fail_send,
            accept_after,
            contact_checks: AtomicU32::new(0),
        })
    }
}

#[async_trait]
impl Relay for FakeWorker {
    async fn send(&self, _to: &str, body: &str) -> Result<(), String> {
        if self.fail_send {
            return Err("send boom".to_string());
        }
        let frame = decode_task_frame(body).expect("runner sends a valid task frame");
        self.sent.lock().await.push(frame.kind.as_str().to_string());
        // Only a `task` frame starts work. An `abort` is the runner telling us to
        // stop one, and queues nothing.
        if frame.kind != TaskFrameKind::Task {
            return Ok(());
        }
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
        // A message the pump cannot decode must be skipped, not fatal — queue one
        // ahead of everything so the pump's skip-and-continue path runs first.
        if let Mode::GarbageThenReply(_) = self.mode {
            q.push_back(InboundMessage {
                from: "worker".to_string(),
                text: "not-a-task-frame".to_string(),
            });
        }
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
            Mode::GarbageThenReply(text) => q.push_back(mk(TaskFrameKind::Reply, text, None)),
            Mode::Chatty { statuses, reply } => {
                for n in 0..*statuses {
                    q.push_back(mk(TaskFrameKind::Status, &format!("working {n}"), None));
                }
                q.push_back(mk(TaskFrameKind::Reply, reply, None));
            }
            // Ack + status already queued above; no terminal frame follows.
            Mode::Silent | Mode::AckOnly => {}
        }
        Ok(())
    }

    async fn drain_inbox(&self, limit: i64) -> Vec<InboundMessage> {
        // One frame per drain in `Chatty`, so the stream is spread across poll
        // intervals rather than arriving in a single burst — otherwise every
        // frame lands inside the first idle budget and proves nothing.
        let limit = match self.mode {
            Mode::Chatty { .. } => 1,
            _ => limit,
        };
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

    /// Accepted once polled `accept_after` times (0 → already a contact, so `run`
    /// proceeds straight to the send).
    async fn contact_accepted(&self, _peer: &str) -> bool {
        self.contact_checks.fetch_add(1, Ordering::Relaxed) >= self.accept_after
    }

    async fn reset_session(&self, _peer: &str) {
        self.resets.fetch_add(1, Ordering::Relaxed);
    }
}

/// A dispatch request the tests mutate per case. `abort_id` mirrors `task_id` so a
/// test can abort by the same id it dispatched under.
pub(super) fn req(instruction: &str) -> TaskRequest {
    TaskRequest {
        task_id: "t1".to_string(),
        abort_id: "t1".to_string(),
        cycle_id: Some("c1".to_string()),
        instruction: instruction.to_string(),
        worker_address: "GRV1worker".to_string(),
        provider: None,
        model: None,
    }
}
