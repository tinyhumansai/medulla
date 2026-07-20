//! The tiny.place task-sender runner — the outbound half of the harness plane.
//!
//! `SignalTransport`/the daemon only ever RECEIVE task frames; this runner SENDS
//! them. It dispatches a `task` frame to a worker over Signal DMs, then routes
//! the worker's `ack`/`status`/`reply`/`error` frames back to the awaiting
//! caller — correlated by a per-dispatch `correlationId`, because the inbox is
//! shared across concurrent dispatches (draining it acknowledges every message,
//! so one pump must fan each frame out to the right waiter).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot, Mutex, Notify};

use crate::tinyplace::{
    decode_task_frame, encode_task_frame, EncodeFrameInput, TaskFrame, TaskFrameKind, TokenUsage,
};

use super::relay::Relay;
use super::types::{RunError, TaskOutcome, TaskRequest};

/// How many inbound messages to drain per pump tick.
const DRAIN_LIMIT: i64 = 50;

/// How long to wait for a peer to accept our contact request before sending.
const CONTACT_WAIT: Duration = Duration::from_secs(20);
/// How often to re-check contact status while waiting.
const CONTACT_POLL: Duration = Duration::from_millis(500);

/// How long to wait for the FIRST sign of life (any inbound frame — `ack`,
/// `status`, `reply`, `error`) before treating the peer as unreachable and
/// re-handshaking. Short: a live worker acks within a poll or two.
const ACK_WINDOW: Duration = Duration::from_secs(12);
/// How many times to reset the Signal session + resend before giving up. Covers
/// the common one-sided-session desync (worker restarted) in one extra round.
const MAX_RESETS: u32 = 2;

/// A registered dispatch awaiting its terminal frame.
struct Waiter {
    /// Resolved with the terminal outcome (`reply`) or the worker's error text.
    reply: oneshot::Sender<Result<TaskOutcome, String>>,
    /// Optional progress sink fed by `status` frames while the task runs.
    status: Option<mpsc::UnboundedSender<String>>,
    /// Notified on ANY inbound frame for this dispatch — the "peer is alive"
    /// signal the runner's ack window waits on.
    activity: Arc<Notify>,
}

/// Shared registry of in-flight dispatches, keyed by `correlationId`.
type Waiters = Arc<Mutex<HashMap<String, Waiter>>>;

/// Route one decoded frame to its waiter, keyed by `correlationId` (falling back
/// to `taskId`). Any frame pokes the waiter's `activity` (sign of life);
/// `reply`/`error` then settle and remove it; `status` forwards; `ack` just
/// counted as activity.
async fn route_frame(waiters: &Waiters, frame: TaskFrame) {
    let key = frame
        .correlation_id
        .clone()
        .unwrap_or_else(|| frame.task_id.clone());
    // One lock for the whole routing — every op below is synchronous.
    let mut map = waiters.lock().await;
    if let Some(w) = map.get(&key) {
        w.activity.notify_one();
    }
    match frame.kind {
        TaskFrameKind::Reply => {
            if let Some(w) = map.remove(&key) {
                let _ = w.reply.send(Ok(TaskOutcome {
                    reply: frame.text,
                    usage: frame.usage.unwrap_or(TokenUsage {
                        input_tokens: 0,
                        output_tokens: 0,
                    }),
                    harness: frame.harness,
                }));
            }
        }
        TaskFrameKind::Error => {
            if let Some(w) = map.remove(&key) {
                let _ = w.reply.send(Err(frame.text));
            }
        }
        TaskFrameKind::Status => {
            if let Some(w) = map.get(&key) {
                if let Some(tx) = &w.status {
                    let _ = tx.send(frame.text);
                }
            }
        }
        // ack / task / input / capabilities* — activity already recorded.
        _ => {}
    }
}

/// The pump: drain the inbox, decode each message, route it, then sleep. Runs
/// until the owning [`TaskRunner`] is dropped (which aborts the task).
async fn pump_loop(relay: Arc<dyn Relay>, waiters: Waiters, poll: Duration) {
    loop {
        for msg in relay.drain_inbox(DRAIN_LIMIT).await {
            if let Some(frame) = decode_task_frame(&msg.text) {
                route_frame(&waiters, frame).await;
            }
        }
        tokio::time::sleep(poll).await;
    }
}

/// Sends tasks to remote tiny.place workers and correlates their replies.
///
/// Holds a shared [`Relay`] and a background pump that drains the encrypted
/// inbox and fans decoded frames to per-dispatch waiters. Wrap in `Arc` to share
/// across dispatches; dropping it aborts the pump.
pub struct TaskRunner {
    relay: Arc<dyn Relay>,
    waiters: Waiters,
    counter: AtomicU64,
    /// How long a dispatch waits for the first sign of life before re-handshaking.
    ack_window: Duration,
    pump: tokio::task::JoinHandle<()>,
}

impl Drop for TaskRunner {
    fn drop(&mut self) {
        self.pump.abort();
    }
}

impl TaskRunner {
    /// Start a runner over `relay`, spawning the inbox pump that polls every
    /// `poll` interval, with the default [`ACK_WINDOW`].
    pub fn start(relay: Arc<dyn Relay>, poll: Duration) -> Self {
        Self::start_with_ack_window(relay, poll, ACK_WINDOW)
    }

    /// Like [`start`](Self::start) with an explicit ack window (tests use a short
    /// one to exercise the reset-and-resend recovery without real delays).
    pub fn start_with_ack_window(
        relay: Arc<dyn Relay>,
        poll: Duration,
        ack_window: Duration,
    ) -> Self {
        let waiters: Waiters = Arc::new(Mutex::new(HashMap::new()));
        let pump = tokio::spawn(pump_loop(relay.clone(), waiters.clone(), poll));
        TaskRunner {
            relay,
            waiters,
            counter: AtomicU64::new(0),
            ack_window,
            pump,
        }
    }

    /// Dispatch `req` to its worker and await the terminal `reply`/`error`, with
    /// automatic recovery from a desynced session.
    ///
    /// Requests a contact first (idempotent; a peer refuses a DM before one
    /// exists). Then, per attempt: encode a `task` frame under a fresh
    /// `correlationId`, send it, and wait an `ACK_WINDOW` for the FIRST sign of
    /// life. If the peer answers (any frame), await the terminal reply for the
    /// full `req.timeout`. If the peer is silent — the classic one-sided session
    /// after a worker restart, where our `CIPHERTEXT` is undecryptable and
    /// dropped — reset the Signal session (forcing a fresh X3DH) and resend, up
    /// to `MAX_RESETS`. `status` frames are forwarded to `status` throughout.
    pub async fn run(
        &self,
        req: TaskRequest,
        status: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<TaskOutcome, RunError> {
        // Establish the contact and WAIT for acceptance. A request only creates a
        // `pending` edge, and the relay refuses a DM to a non-contact
        // (`403 not_a_contact`) — sending immediately races the peer's
        // auto-accepter. Bounded, so a peer that never accepts surfaces as a
        // normal task error instead of hanging.
        if !self.relay.contact_accepted(&req.worker_address).await {
            let _ = self.relay.request_contact(&req.worker_address).await;
            let deadline = std::time::Instant::now() + CONTACT_WAIT;
            while std::time::Instant::now() < deadline
                && !self.relay.contact_accepted(&req.worker_address).await
            {
                tokio::time::sleep(CONTACT_POLL).await;
            }
        }

        let mut attempt = 0u32;
        loop {
            let cid = format!(
                "{}/{}/{}",
                req.cycle_id.as_deref().unwrap_or("cyc"),
                req.task_id,
                self.counter.fetch_add(1, Ordering::Relaxed)
            );
            let (tx, mut rx) = oneshot::channel();
            let activity = Arc::new(Notify::new());
            self.waiters.lock().await.insert(
                cid.clone(),
                Waiter {
                    reply: tx,
                    status: status.clone(),
                    activity: activity.clone(),
                },
            );

            let body = encode_task_frame(EncodeFrameInput {
                kind: TaskFrameKind::Task,
                task_id: req.task_id.clone(),
                text: req.instruction.clone(),
                ts: ::tinyplace::auth::timestamp(),
                correlation_id: Some(cid.clone()),
                harness: None,
                provider: req.provider,
                model: req.model.clone(),
            });

            if let Err(e) = self.relay.send(&req.worker_address, &body).await {
                self.waiters.lock().await.remove(&cid);
                return Err(RunError::Transport(e));
            }

            // Ack window: first sign of life, an early terminal, or silence.
            tokio::select! {
                biased;
                terminal = &mut rx => return settle(terminal),
                _ = activity.notified() => {
                    // Peer is alive — await the terminal reply for the full timeout.
                    return match tokio::time::timeout(req.timeout, rx).await {
                        Ok(terminal) => settle(terminal),
                        Err(_) => {
                            self.waiters.lock().await.remove(&cid);
                            Err(RunError::Timeout)
                        }
                    };
                }
                _ = tokio::time::sleep(self.ack_window) => {
                    // Silence — the peer likely can't decrypt (restarted / one-sided
                    // session). Reset and resend, or give up.
                    self.waiters.lock().await.remove(&cid);
                    if attempt >= MAX_RESETS {
                        return Err(RunError::Timeout);
                    }
                    attempt += 1;
                    self.relay.reset_session(&req.worker_address).await;
                }
            }
        }
    }
}

/// Map the oneshot outcome into a [`RunError`].
fn settle(
    terminal: Result<Result<TaskOutcome, String>, oneshot::error::RecvError>,
) -> Result<TaskOutcome, RunError> {
    match terminal {
        Ok(Ok(outcome)) => Ok(outcome),
        Ok(Err(msg)) => Err(RunError::Worker(msg)),
        Err(_) => Err(RunError::Transport("dispatch waiter dropped".into())),
    }
}
