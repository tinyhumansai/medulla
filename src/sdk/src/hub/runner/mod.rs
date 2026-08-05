//! The bridge-independent task sender — the outbound half of the harness plane.
//!
//! The daemon only ever receives task frames; this runner sends them. It
//! dispatches a `task` frame over the selected local or link bridge, then
//! routes the worker's `ack`/`status`/`reply`/`error` frames back to the awaiting
//! caller. Frames are correlated by a per-dispatch `correlationId`, because the
//! inbox is shared across concurrent dispatches and one pump must fan each frame
//! out to the right waiter.
//!
//! The inbound routing lives in [`pump`]; this module owns dispatch, the liveness
//! bounds, and orchestrator-driven abort.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot, Mutex, Notify};

use crate::protocol::{
    encode_task_frame, AgentCapabilities, EncodeFrameInput, TaskFrameKind, WorkerSystemInfo,
};

use crate::bridge::BridgeLiveness;

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
///
/// It is deliberately shorter than the runtime a worker permits a task, and that
/// is only sound because the worker heartbeats: a task waiting for a harness
/// slot, or grinding through one long tool call, emits a status frame every
/// heartbeat period (a small multiple of its status throttle, capped at 60s)
/// even when the harness itself says nothing. Silence past this window therefore
/// means the worker is gone, not that the work is slow. Shortening the worker's
/// heartbeat guarantee without shortening this window — or lengthening it past
/// this window — brings back the failure this pairing exists to prevent: healthy
/// dispatches reaped as `bridge task timed out`.
const IDLE_WINDOW: Duration = Duration::from_secs(240);

/// How many times to reset the transport session + resend before giving up.
/// Covers a peer that has genuinely stopped listening in one extra round.
const MAX_RESETS: u32 = 2;

/// How often [`live_sleep`] re-reads link liveness while a window is running.
///
/// Short enough that a link coming back is noticed promptly, long enough that a
/// four-minute idle window costs a few thousand cheap status reads rather than
/// a busy loop.
const LIVENESS_TICK: Duration = Duration::from_millis(100);

/// Sleep for `window` of *link-live* time (host-link protocol §6.3).
///
/// This is the single most important integration detail of the host link, so it
/// is a function rather than an inline sleep. Both of this runner's clocks —
/// [`ACK_WINDOW`] and [`IDLE_WINDOW`] — exist because the old mailbox transport
/// could silently black-hole a frame. The link cannot: it owns retransmission
/// and recovers an outage by itself, with no reconnect and no re-enrollment. So
/// a 30-second blip must not fail a task the transport was in the middle of
/// recovering — which is exactly what an ordinary `sleep` would do.
///
/// Time therefore only accrues while the link to `peer` reports
/// [`BridgeLiveness::Live`]. When it does not, the window is **paused**, and it
/// **resumes rather than resets** on recovery: `ACK_WINDOW` measures peer
/// *processing*, and a peer that was unreachable was not thinking. The
/// distinction matters in the other direction too — a peer that is hung on a
/// `Live` link still times out on schedule, because the clock was gated, not
/// disabled.
///
/// The gate is evaluated for `peer`, never for the link as a whole (§6.3): an
/// orchestrator holds sessions with many hosts, and one laptop going to sleep
/// must not stop every other host's clock.
///
/// A bridge with no notion of reachability (the in-memory bus, every test fake)
/// answers `Live` by default, so this behaves exactly like `sleep` for them.
///
/// `held` is the second gate, and it is the same idea one layer up: the worker
/// reports (`crate::daemon::SESSION_HELD_STATUS_PREFIX`) that an operator has
/// taken the session serving this dispatch, and a person reading their own
/// session is no more "a crashed worker" than an unreachable one is. Held time
/// therefore does not accrue either, and the window **resumes rather than
/// resets** when the session is handed back — a worker that dies mid-hold is
/// still given up on, it just is not given up on *while* a human has it.
async fn live_sleep(relay: &dyn Relay, peer: &str, window: Duration, held: &AtomicBool) {
    let mut remaining = window;
    while !remaining.is_zero() {
        if relay.liveness(peer).await == BridgeLiveness::Live && !held.load(Ordering::Acquire) {
            let step = LIVENESS_TICK.min(remaining);
            tokio::time::sleep(step).await;
            remaining -= step;
        } else {
            // Paused: wait out a tick without spending any of the window.
            tokio::time::sleep(LIVENESS_TICK).await;
        }
    }
}

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
    /// Number of registered capability probes, exposed only for cleanup tests.
    #[cfg(test)]
    pub(in crate::hub) fn capability_waiter_count(&self) -> usize {
        self.capabilities_waiters.lock().unwrap().len()
    }

    /// The relay this runner dispatches over, for callers that need to ask it
    /// something — currently roster liveness.
    pub fn relay(&self) -> Arc<dyn Relay> {
        self.relay.clone()
    }

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

    /// The screens this hub is holding, for the view that renders them.
    pub fn screens(&self) -> super::ScreenStore {
        self.screens.clone()
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
        let capabilities_waiters: CapabilitiesWaiters =
            Arc::new(std::sync::Mutex::new(HashMap::new()));
        let screens = super::ScreenStore::new();
        let pump = tokio::spawn(pump::pump_loop(
            relay.clone(),
            waiters.clone(),
            system_info_waiters.clone(),
            capabilities_waiters.clone(),
            poll,
            log,
            activity,
            screens.clone(),
        ));
        TaskRunner {
            relay,
            screens,
            waiters,
            system_info_waiters,
            capabilities_waiters,
            capabilities: Arc::new(Mutex::new(HashMap::new())),
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
    /// dispatch to its own liveness bound. Returns whether a live dispatch was
    /// found and signalled.
    pub fn abort_task(&self, task_id: &str) -> bool {
        let Ok(mut aborts) = self.aborts.lock() else {
            return false;
        };
        let Some(signal) = aborts.remove(task_id) else {
            return false;
        };
        // Removing and notifying under the same lock makes cancellation and
        // AbortGuard's terminal cleanup mutually exclusive. If cleanup wins,
        // this returns false; if this wins, a live task receives the signal.
        signal.notify_one();
        true
    }

    /// Return the active dispatch receipt for a worker/task pair.
    pub async fn kill_correlation_for(
        &self,
        worker: &str,
        task_id: &str,
    ) -> Option<(String, String)> {
        self.waiters
            .lock()
            .await
            .iter()
            .find(|(_, waiter)| {
                waiter.from == worker && waiter.task_id == task_id && waiter.screen_kill
            })
            .map(|(correlation, waiter)| (correlation.clone(), waiter.wire_task_id.clone()))
    }

    /// Cancel every dispatch this runner has in flight.
    ///
    /// For a caller that owns a runner serving one piece of work and wants to
    /// stop it without having kept the task id — the copilot pane, whose runner
    /// serves one conversation. On a runner shared by unrelated work this would
    /// be far too broad, which is why nothing shared calls it.
    ///
    /// Best-effort in the same way [`abort_task`](Self::abort_task) is: a
    /// poisoned lock leaves each dispatch to its own liveness bound.
    pub fn abort_all(&self) {
        let signals: Vec<_> = self
            .aborts
            .lock()
            .ok()
            .map(|map| map.values().cloned().collect())
            .unwrap_or_default();
        for signal in signals {
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
    /// dispatch can never pin its correlation entry: the `ACK_WINDOW` on a
    /// never-answering peer, and — once alive — the `IDLE_WINDOW` no-progress
    /// watchdog, which reaps a worker that acks then stops emitting frames
    /// (crashed / vanished). Neither is a wall-clock cap on a working peer, and
    /// both are measured in *link-live* time only — see [`live_sleep`].
    pub async fn run(
        &self,
        req: TaskRequest,
        status: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<TaskOutcome, RunError> {
        self.run_inner(req, status, false, None, None).await
    }

    /// Run a dispatch with the screen-control support negotiated specifically
    /// for this request.
    pub async fn run_negotiated(
        &self,
        req: TaskRequest,
        status: Option<mpsc::UnboundedSender<String>>,
        screen_kill: bool,
        abort: Option<Arc<Notify>>,
        visible_task_id: Option<String>,
    ) -> Result<TaskOutcome, RunError> {
        self.run_inner(req, status, screen_kill, abort, visible_task_id)
            .await
    }

    async fn run_inner(
        &self,
        req: TaskRequest,
        status: Option<mpsc::UnboundedSender<String>>,
        screen_kill: bool,
        prepared_abort: Option<Arc<Notify>>,
        visible_task_id: Option<String>,
    ) -> Result<TaskOutcome, RunError> {
        // Register this dispatch's abort signal FIRST — before the contact wait —
        // so a `task_abort` that arrives during contact negotiation (up to
        // `CONTACT_WAIT` for a first-time worker) is honored, not silently dropped
        // by finding nothing in the registry. Keyed by the orchestrator-facing id
        // the backend aborts by, and held for the whole call (spanning any
        // reset+resend retries). The guard removes it on every return path, so a
        // settled dispatch leaves nothing for a later `task_abort` to match.
        let abort = prepared_abort.unwrap_or_else(|| Arc::new(Notify::new()));
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
            // Held for the whole attempt, not only for as long as the waiter is
            // registered: the windows below read it, and the pump writes it.
            let held = Arc::new(AtomicBool::new(false));
            self.waiters.lock().await.insert(
                cid.clone(),
                Waiter {
                    task_id: visible_task_id
                        .clone()
                        .unwrap_or_else(|| req.task_id.clone()),
                    wire_task_id: req.task_id.clone(),
                    screen_kill,
                    from: req.worker_address.clone(),
                    reply: tx,
                    status: status.clone(),
                    activity: activity.clone(),
                    held: held.clone(),
                },
            );

            let body = encode_task_frame(EncodeFrameInput {
                kind: TaskFrameKind::Task,
                task_id: req.task_id.clone(),
                text: req.instruction.clone(),
                ts: crate::clock::iso_now(),
                correlation_id: Some(cid.clone()),
                harness: None,
                provider: req.provider,
                custom_harness: req.custom_harness.clone(),
                model: req.model.clone(),
                tool_mode: req.tool_mode.clone(),
                workflow: req.workflow.clone(),
                workflow_fingerprint: req.workflow_fingerprint.clone(),
                workflow_inputs: req.workflow_inputs.clone(),
                conversation: req.conversation.clone(),
                fleet_depth: req.fleet_depth,
            });

            tokio::select! {
                biased;
                _ = abort.notified() => {
                    self.waiters.lock().await.remove(&cid);
                    return Err(RunError::Aborted);
                }
                result = self.relay.send(&req.worker_address, &body) => {
                    if let Err(e) = result {
                        self.waiters.lock().await.remove(&cid);
                        return Err(RunError::Transport(e));
                    }
                }
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
                            _ = live_sleep(
                                self.relay.as_ref(), &req.worker_address, self.idle_window, &held,
                            ) => {
                                self.waiters.lock().await.remove(&cid);
                                send_abort(
                                    self.relay.as_ref(), &req.worker_address, &req.task_id, &cid,
                                ).await;
                                return Err(RunError::Timeout);
                            }
                        }
                    }
                }
                _ = live_sleep(
                    self.relay.as_ref(), &req.worker_address, self.ack_window, &held,
                ) => {
                    // Silence while the link was live — so the peer itself is not
                    // answering, not the network. Reset and resend, or give up.
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
        ts: crate::clock::iso_now(),
        correlation_id: Some(cid.to_string()),
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
    });
    let _ = relay.send(address, &body).await;
}

/// Map the oneshot outcome into a [`RunError`].
///
/// A worker's load-shed rejection is separated out here rather than at the far
/// end: it arrives as an ordinary `error` frame, but it means "I did not try
/// this", not "this failed". Recognised by the prefix the daemon builds every
/// such message from, so the two ends share one constant instead of a copied
/// literal.
fn settle(
    terminal: Result<Result<TaskOutcome, String>, oneshot::error::RecvError>,
) -> Result<TaskOutcome, RunError> {
    match terminal {
        Ok(Ok(outcome)) => Ok(outcome),
        Ok(Err(msg)) if msg.starts_with(crate::daemon::CAPACITY_REJECTION_PREFIX) => {
            Err(RunError::Busy(msg))
        }
        Ok(Err(msg)) if msg.starts_with(crate::daemon::HARNESS_HELD_PREFIX) => {
            Err(RunError::Held(msg))
        }
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
