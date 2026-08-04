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

use crate::bridge::InboundMessage;
use crate::hub::{Relay, TaskRequest};
use crate::protocol::{
    decode_task_frame, encode_task_frame_with_attachments, encode_task_frame_with_usage,
    AgentCapabilities, EncodeFrameInput, FrameAttachments, TaskFrameKind, TokenUsage,
    WorkerSystemInfo,
};

/// The harness session this fake worker reports on every terminal `reply`.
///
/// A real daemon opens exactly one session per task and names it on the reply,
/// which is the only way the id ever travels back up. Stamping it here keeps the
/// fake honest about that.
pub(in crate::hub::tests) const FAKE_SESSION_ID: &str = "sess-fake-01";

impl FakeWorker {
    /// The kinds of frame the runner has sent us, in order.
    pub(in crate::hub::tests) async fn sent_kinds(&self) -> Vec<String> {
        self.sent.lock().await.clone()
    }

    pub(in crate::hub::tests) fn new(mode: Mode) -> Arc<Self> {
        Self::with(mode, false, 0)
    }

    /// A worker with explicit send-failure and contact-acceptance-delay knobs.
    pub(in crate::hub::tests) fn with(mode: Mode, fail_send: bool, accept_after: u32) -> Arc<Self> {
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
    async fn send(&self, to: &str, body: &str) -> Result<(), String> {
        if self.fail_send {
            return Err("send boom".to_string());
        }
        let frame = decode_task_frame(body).expect("runner sends a valid task frame");
        self.sent.lock().await.push(frame.kind.as_str().to_string());
        if frame.kind == TaskFrameKind::SystemInfo {
            let text = match &self.mode {
                Mode::SystemInfo(info) => serde_json::to_string(info).unwrap(),
                Mode::SystemInfoAfterReset(info) if self.resets.load(Ordering::Relaxed) > 0 => {
                    serde_json::to_string(info).unwrap()
                }
                Mode::InvalidSystemInfo => "{not-json".to_string(),
                Mode::Silent | Mode::SystemInfoAfterReset(_) => return Ok(()),
                _ => return Ok(()),
            };
            self.inbox.lock().await.push_back(InboundMessage {
                from: to.to_string(),
                text: encode_task_frame_with_usage(
                    EncodeFrameInput {
                        kind: TaskFrameKind::SystemInfoResult,
                        task_id: frame.task_id,
                        text,
                        ts: "T".to_string(),
                        correlation_id: frame.correlation_id,
                        harness: None,
                        provider: None,
                        custom_harness: None,
                        model: None,
                        tool_mode: None,
                        workflow: None,
                        workflow_fingerprint: None,
                        workflow_inputs: Default::default(),
                        conversation: None,
                        fleet_depth: 0,
                    },
                    None,
                ),
            });
            return Ok(());
        }
        if frame.kind == TaskFrameKind::Capabilities {
            if let Mode::Capabilities(caps) = &self.mode {
                self.inbox.lock().await.push_back(InboundMessage {
                    from: to.to_string(),
                    text: encode_task_frame_with_usage(
                        EncodeFrameInput {
                            kind: TaskFrameKind::CapabilitiesResult,
                            task_id: frame.task_id,
                            text: serde_json::to_string(caps).unwrap(),
                            ts: "T".to_string(),
                            correlation_id: frame.correlation_id,
                            harness: None,
                            provider: None,
                            custom_harness: None,
                            model: None,
                            tool_mode: None,
                            workflow: None,
                            workflow_fingerprint: None,
                            workflow_inputs: Default::default(),
                            conversation: None,
                            fleet_depth: 0,
                        },
                        None,
                    ),
                });
            }
            return Ok(());
        }
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
            from: to.to_string(),
            text: encode_task_frame_with_usage(
                EncodeFrameInput {
                    kind,
                    task_id: task_id.clone(),
                    text: text.to_string(),
                    ts: "T".to_string(),
                    correlation_id: cid.clone(),
                    harness: None,
                    provider: None,
                    custom_harness: None,
                    model: None,
                    tool_mode: None,
                    workflow: None,
                    workflow_fingerprint: None,
                    workflow_inputs: Default::default(),
                    conversation: None,
                    fleet_depth: 0,
                },
                usage,
            ),
        };
        // A terminal reply names the session that served the task; nothing else
        // does. `mk` stays session-free so an ack or a status cannot claim one.
        let reply = |text: &str, usage| InboundMessage {
            from: to.to_string(),
            text: encode_task_frame_with_attachments(
                EncodeFrameInput {
                    kind: TaskFrameKind::Reply,
                    task_id: task_id.clone(),
                    text: text.to_string(),
                    ts: "T".to_string(),
                    correlation_id: cid.clone(),
                    harness: None,
                    provider: None,
                    custom_harness: None,
                    model: None,
                    tool_mode: None,
                    workflow: None,
                    workflow_fingerprint: None,
                    workflow_inputs: Default::default(),
                    conversation: None,
                    fleet_depth: 0,
                },
                FrameAttachments {
                    usage,
                    work: None,
                    session_id: Some(FAKE_SESSION_ID.to_string()),
                },
            ),
        };
        let mut q = self.inbox.lock().await;
        // A message the pump cannot decode must be skipped, not fatal — queue one
        // ahead of everything so the pump's skip-and-continue path runs first.
        if let Mode::GarbageThenReply(_) = self.mode {
            q.push_back(InboundMessage {
                from: to.to_string(),
                text: "not-a-task-frame".to_string(),
            });
        }
        // An impostor's frames go in ahead of the worker's own, carrying the right
        // correlation id from the wrong sender.
        if let Mode::ImpostorThenReply {
            impostor, stolen, ..
        } = &self.mode
        {
            for kind in [TaskFrameKind::Status, TaskFrameKind::Reply] {
                q.push_back(InboundMessage {
                    from: impostor.clone(),
                    text: encode_task_frame_with_usage(
                        EncodeFrameInput {
                            kind,
                            task_id: task_id.clone(),
                            text: stolen.clone(),
                            ts: "T".to_string(),
                            correlation_id: cid.clone(),
                            harness: None,
                            provider: None,
                            custom_harness: None,
                            model: None,
                            tool_mode: None,
                            workflow: None,
                            workflow_fingerprint: None,
                            workflow_inputs: Default::default(),
                            conversation: None,
                            fleet_depth: 0,
                        },
                        None,
                    ),
                });
            }
        }
        q.push_back(mk(TaskFrameKind::Ack, "accepted", None));
        q.push_back(mk(TaskFrameKind::Status, "running python audit.py", None));
        match &self.mode {
            Mode::ImpostorThenReply { reply: text, .. }
            | Mode::Reply(text)
            | Mode::RecoverAfterReset(text) => q.push_back(reply(
                text,
                Some(TokenUsage {
                    input_tokens: 3,
                    output_tokens: 5,
                }),
            )),
            Mode::Error(text) => q.push_back(mk(TaskFrameKind::Error, text, None)),
            Mode::GarbageThenReply(text) => q.push_back(reply(text, None)),
            Mode::Chatty {
                statuses,
                reply: reply_text,
            } => {
                for n in 0..*statuses {
                    q.push_back(mk(TaskFrameKind::Status, &format!("working {n}"), None));
                }
                q.push_back(reply(reply_text, None));
            }
            // Ack + status already queued above; no terminal frame follows.
            Mode::Silent | Mode::AckOnly => {}
            Mode::SystemInfo(_) | Mode::SystemInfoAfterReset(_) | Mode::InvalidSystemInfo => {}
            Mode::Capabilities(_) => {}
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
pub(in crate::hub::tests) fn req(instruction: &str) -> TaskRequest {
    TaskRequest {
        task_id: "t1".to_string(),
        abort_id: "t1".to_string(),
        cycle_id: Some("c1".to_string()),
        instruction: instruction.to_string(),
        worker_address: "GRV1worker".to_string(),
        provider: None,
        custom_harness: None,
        model: None,
        tool_mode: None,
        workflow: None,
        workflow_fingerprint: None,
        workflow_inputs: Default::default(),
        conversation: None,
        fleet_depth: 0,
    }
}

mod types;
pub(in crate::hub::tests) use types::FakeWorker;
pub(in crate::hub::tests) use types::Mode;
