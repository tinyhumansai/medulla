//! The bridge-independent task sender — the outbound half of the harness plane.
//!
//! The daemon only ever receives task frames; this runner sends them. It
//! dispatches a `task` frame over the selected local or tiny.place bridge, then
//! routes the worker's `ack`/`status`/`reply`/`error` frames back to the awaiting
//! caller. Frames are correlated by a per-dispatch `correlationId`, because the
//! inbox is shared across concurrent dispatches and one pump must fan each frame
//! out to the right waiter.
//!
//! The inbound routing lives in [`pump`]; this module owns dispatch, the liveness
//! bounds, and orchestrator-driven abort.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot, Mutex, Notify};

use crate::tinyplace::{
    encode_task_frame, AgentCapabilities, EncodeFrameInput, TaskFrameKind, WorkerSystemInfo,
};

use super::relay::Relay;
use super::types::{RunError, TaskOutcome, TaskRequest};

mod capabilities;
mod pump;
mod system_info;

/// How long to wait for a peer to accept our contact request before sending.
const CONTACT_WAIT: Duration = Duration::from_secs(20);
/// How often to re-check contact status while waiting.
const CONTACT_POLL: Duration = Duration::from_millis(500);

/// How long to wait for the FIRST sign of life (any inbound frame — `ack`,
/// `status`, `reply`, `error`) before treating the peer as unreachable and
/// re-handshaking. Short: a live worker acks within a poll or two.
const ACK_WINDOW: Duration = Duration::from_secs(12);

/// The no-progress (liveness) window, applied only AFTER the peer is alive: how
/// long a dispatch may receive NO inbound frame before the hub treats the worker
/// as dead, reaps the correlation entry, and gives up. Reset by every frame, so a
/// worker that keeps emitting `status` is never given up on however long it runs.
///
/// This is a liveness bound, NOT a task deadline. The orchestrator owns the
/// deadline — "how long may this task take" — and aborts a running task in sync
/// mode via `medulla:task_abort`. The hub's only job here is to make sure a
/// crashed or vanished worker (which stops sending frames) cannot pin its
/// correlation entry and spawned handler forever: without this bound, a worker
/// that acks and then dies before sending a terminal frame would leak.
const IDLE_WINDOW: Duration = Duration::from_secs(240);

/// How many times to reset the Signal session + resend before giving up. Covers
/// the common one-sided-session desync (worker restarted) in one extra round.
const MAX_RESETS: u32 = 2;

impl Drop for AbortGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = self.aborts.lock() {
            if map
                .get(&self.key)
                .is_some_and(|s| Arc::ptr_eq(s, &self.signal))
            {
                map.remove(&self.key);
            }
        }
    }
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

    /// Start a runner that narrates every inbound worker frame to `log`.
    ///
    /// The sink is passed at construction rather than attached afterwards: the
    /// pump begins draining the moment it is spawned, and restarting it to add a
    /// logger would leave a window in which a frame is consumed unlogged — and,
    /// with no waiter registered yet, dropped.
    pub fn start_with_log(
        relay: Arc<dyn Relay>,
        poll: Duration,
        log: super::types::HubLog,
    ) -> Self {
        Self::build(relay, poll, ACK_WINDOW, IDLE_WINDOW, Some(log), None)
    }

    /// Like [`start_with_log`](Self::start_with_log), also recording what each
    /// worker does so the Agents view can render it.
    pub fn start_with_log_and_activity(
        relay: Arc<dyn Relay>,
        poll: Duration,
        log: super::types::HubLog,
        activity: super::ActivityLog,
    ) -> Self {
        Self::build(
            relay,
            poll,
            ACK_WINDOW,
            IDLE_WINDOW,
            Some(log),
            Some(activity),
        )
    }

    /// Like [`start`](Self::start) with an explicit ack window (tests use a short
    /// one to exercise the reset-and-resend recovery without real delays).
    pub fn start_with_ack_window(
        relay: Arc<dyn Relay>,
        poll: Duration,
        ack_window: Duration,
    ) -> Self {
        Self::build(relay, poll, ack_window, IDLE_WINDOW, None, None)
    }

    /// Like [`start`](Self::start) with an explicit no-progress window (tests use
    /// a short one to exercise the liveness watchdog — a worker that acks then
    /// goes silent — without real delays).
    pub fn start_with_idle_window(
        relay: Arc<dyn Relay>,
        poll: Duration,
        idle_window: Duration,
    ) -> Self {
        Self::build(relay, poll, ACK_WINDOW, idle_window, None, None)
    }

    fn build(
        relay: Arc<dyn Relay>,
        poll: Duration,
        ack_window: Duration,
        idle_window: Duration,
        log: Option<super::types::HubLog>,
        activity: Option<super::ActivityLog>,
    ) -> Self {
        let waiters: Waiters = Arc::new(Mutex::new(HashMap::new()));
        let system_info_waiters: SystemInfoWaiters = Arc::new(Mutex::new(HashMap::new()));
        let capabilities_waiters: CapabilitiesWaiters = Arc::new(Mutex::new(HashMap::new()));
        let pump = tokio::spawn(pump::pump_loop(
            relay.clone(),
            waiters.clone(),
            system_info_waiters.clone(),
            capabilities_waiters.clone(),
            poll,
            log,
            activity,
        ));
        TaskRunner {
            relay,
            waiters,
            system_info_waiters,
            capabilities_waiters,
            aborts: Arc::new(std::sync::Mutex::new(HashMap::new())),
            counter: AtomicU64::new(0),
            ack_window,
            idle_window,
            pump,
        }
    }

    /// Cancel the in-flight dispatch for `task_id` (the orchestrator-facing id the
    /// backend aborts by, `medulla:task_abort.taskId`).
    ///
    /// Wakes that dispatch's [`run`](Self::run) so it tells the worker to stop,
    /// reaps its correlation entry, and returns [`RunError::Aborted`]. A no-op if
    /// no dispatch is in flight for that id — it already settled, or was never
    /// dispatched here. Best-effort: a lost signal (poisoned lock) just leaves the
    /// dispatch to its own liveness bound.
    pub fn abort_task(&self, task_id: &str) {
        let signal = self
            .aborts
            .lock()
            .ok()
            .and_then(|map| map.get(task_id).cloned());
        if let Some(signal) = signal {
            signal.notify_one();
        }
    }

    /// Dispatch `req` to its worker and await the terminal `reply`/`error`, with
    /// automatic recovery from a desynced session.
    ///
    /// Requests a contact first (idempotent; a peer refuses a DM before one
    /// exists). Then, per attempt: encode a `task` frame under a fresh
    /// `correlationId`, send it, and wait an `ACK_WINDOW` for the FIRST sign of
    /// life. If the peer answers (any frame), forward its terminal `reply`/`error`
    /// whenever it arrives — the hub owns no *deadline*, so a worker that keeps
    /// making progress is left to finish however long it runs. If the peer is
    /// silent — the classic one-sided session after a worker restart, where our
    /// `CIPHERTEXT` is undecryptable and dropped — reset the Signal session
    /// (forcing a fresh X3DH) and resend, up to `MAX_RESETS`. `status` frames are
    /// forwarded to `status` throughout.
    ///
    /// The hub is a relay for the task *deadline*: the backend owns "how long may
    /// this take" and aborts a running task in sync mode via `medulla:task_abort`,
    /// which [`abort_task`](Self::abort_task) delivers here — stopping the worker
    /// and returning [`RunError::Aborted`], even while the task is actively
    /// reporting progress (the one path that cancels a healthy, chatty worker).
    /// Beyond that the runner enforces only two *liveness* bounds, so a dead
    /// dispatch can never pin its correlation entry: the [`ACK_WINDOW`] on a
    /// never-answering peer, and — once alive — the [`IDLE_WINDOW`] no-progress
    /// watchdog, which reaps a worker that acks then stops emitting frames
    /// (crashed / vanished). Neither is a wall-clock cap on a working peer.
    pub async fn run(
        &self,
        req: TaskRequest,
        status: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<TaskOutcome, RunError> {
        // Register this dispatch's abort signal FIRST — before the contact wait —
        // so a `task_abort` that arrives during contact negotiation (up to
        // `CONTACT_WAIT` for a first-time worker) is honored, not silently dropped
        // by finding nothing in the registry. Keyed by the orchestrator-facing id
        // the backend aborts by, and held for the whole call (spanning any
        // reset+resend retries). The guard removes it on every return path, so a
        // settled dispatch leaves nothing for a later `task_abort` to match.
        let abort = Arc::new(Notify::new());
        self.aborts
            .lock()
            .expect("aborts lock")
            .insert(req.abort_id.clone(), abort.clone());
        let _abort_guard = AbortGuard {
            aborts: self.aborts.clone(),
            key: req.abort_id.clone(),
            signal: abort.clone(),
        };

        // Establish the contact and WAIT for acceptance. A request only creates a
        // `pending` edge, and the relay refuses a DM to a non-contact
        // (`403 not_a_contact`) — sending immediately races the peer's
        // auto-accepter. Bounded, so a peer that never accepts surfaces as a
        // normal task error instead of hanging. An abort here bails immediately:
        // nothing has been dispatched yet, so there is no worker to stop.
        if !self.relay.contact_accepted(&req.worker_address).await {
            let _ = self.relay.request_contact(&req.worker_address).await;
            let deadline = std::time::Instant::now() + CONTACT_WAIT;
            while std::time::Instant::now() < deadline
                && !self.relay.contact_accepted(&req.worker_address).await
            {
                tokio::select! {
                    biased;
                    _ = abort.notified() => return Err(RunError::Aborted),
                    _ = tokio::time::sleep(CONTACT_POLL) => {}
                }
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
                    from: req.worker_address.clone(),
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
                workflow: req.workflow.clone(),
            });

            if let Err(e) = self.relay.send(&req.worker_address, &body).await {
                self.waiters.lock().await.remove(&cid);
                return Err(RunError::Transport(e));
            }

            // Ack window: first sign of life, an early terminal, an orchestrator
            // abort, or silence.
            tokio::select! {
                biased;
                terminal = &mut rx => return settle(terminal),
                // The backend aborted the task (deadline or `/abort`). Stop the
                // worker and give up, even before it has acked.
                _ = abort.notified() => {
                    self.waiters.lock().await.remove(&cid);
                    send_abort(
                        self.relay.as_ref(), &req.worker_address, &req.task_id, &cid,
                    ).await;
                    return Err(RunError::Aborted);
                }
                _ = activity.notified() => {
                    // Alive. From here the bound is IDLE, not wall-clock: every
                    // frame resets it, so a worker streaming progress is left to
                    // work for as long as it takes and only a SILENT one is given
                    // up on. There is deliberately no hard ceiling — the hub owns
                    // no task deadline. The backend owns "how long may this take"
                    // and aborts a running task via `medulla:task_abort` (handled
                    // below); a hub ceiling here used to kill a task reporting
                    // `running Bash: …` every few seconds at the same moment as one
                    // that had died, and a real coding task crosses that routinely.
                    //
                    // The idle window still fires, because a worker that acks and
                    // then goes silent (crashed / vanished) must not pin its
                    // correlation entry and spawned handler forever — the terminal
                    // frame that would settle this waiter is never coming.
                    loop {
                        tokio::select! {
                            biased;
                            terminal = &mut rx => return settle(terminal),
                            // The backend aborted while the worker was working —
                            // the one case no liveness bound catches, since frames
                            // keep resetting the idle clock. Stop it and give up.
                            _ = abort.notified() => {
                                self.waiters.lock().await.remove(&cid);
                                send_abort(
                                    self.relay.as_ref(), &req.worker_address, &req.task_id, &cid,
                                ).await;
                                return Err(RunError::Aborted);
                            }
                            // A frame: the peer is working. Reset the idle clock.
                            _ = activity.notified() => continue,
                            _ = tokio::time::sleep(self.idle_window) => {
                                self.waiters.lock().await.remove(&cid);
                                send_abort(
                                    self.relay.as_ref(), &req.worker_address, &req.task_id, &cid,
                                ).await;
                                return Err(RunError::Timeout);
                            }
                        }
                    }
                }
                _ = tokio::time::sleep(self.ack_window) => {
                    // Silence — the peer likely can't decrypt (restarted / one-sided
                    // session). Reset and resend, or give up.
                    self.waiters.lock().await.remove(&cid);
                    if attempt >= MAX_RESETS {
                        send_abort(
                            self.relay.as_ref(), &req.worker_address, &req.task_id, &cid,
                        ).await;
                        return Err(RunError::Timeout);
                    }
                    attempt += 1;
                    self.relay.reset_session(&req.worker_address).await;
                }
            }
        }
    }
}

/// Tell a worker to stop a task we have stopped waiting for.
///
/// Best-effort and fire-and-forget: we are already returning an error, and a
/// failed abort must not replace it with a different one. Worth sending even
/// so — an abandoned task keeps a harness busy *and* keeps its id live, and a
/// responder refuses a later task whose id is already running. Unnamed tasks are
/// named positionally, so that id is very often `t1`.
async fn send_abort(relay: &dyn Relay, address: &str, task_id: &str, cid: &str) {
    let body = encode_task_frame(EncodeFrameInput {
        kind: TaskFrameKind::Abort,
        task_id: task_id.to_string(),
        text: "requester stopped waiting".to_string(),
        ts: ::tinyplace::auth::timestamp(),
        correlation_id: Some(cid.to_string()),
        harness: None,
        provider: None,
        model: None,
        workflow: None,
    });
    let _ = relay.send(address, &body).await;
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

mod types;
use types::AbortGuard;
use types::CapabilitiesWaiters;
use types::Probe;
use types::SystemInfoWaiters;
pub use types::TaskRunner;
use types::Waiter;
use types::Waiters;
