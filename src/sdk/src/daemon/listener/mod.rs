//! Envelopes delivered over the relay's push channel, instead of fetched.
//!
//! Every inbound path in medulla used to poll: `GET /messages`, sleep, repeat.
//! That put a floor under latency equal to the poll interval — 1s on a worker,
//! 1.5s on the hub — which a screen stream feels far more than a task frame
//! does.
//!
//! `GET /a2a/{id}/stream` upgrades to a WebSocket that opens with a snapshot of
//! the mailbox and then tails every envelope the relay accepts for us
//! (`PUT /messages` publishes to the same topic). So this holds that socket open
//! and hands the envelopes it carries straight to [`SignalTransport::drain_inbox`],
//! which decrypts and acknowledges them exactly as it did when it fetched them
//! itself.
//!
//! Three things make that safe, and none of them are optional:
//!
//! - **Acknowledgement still goes over HTTPS.** The stream is a fan-out, not a
//!   consumption: the mailbox holds an envelope until `DELETE /messages/{id}`.
//!   Reading one off the socket does not remove it.
//! - **Delivery is deduplicated by message id.** Reconnecting re-sends the
//!   snapshot — up to 50 un-acked envelopes — and a reconciliation fetch can
//!   race a delete that has not landed. Processing a task frame twice is merely
//!   wasteful; processing a *screen delta* twice fails its `base_seq` check and
//!   forces a resync, so this is what keeps the stream from sawing back and
//!   forth.
//! - **The fetch never goes away.** It is skipped while the socket is healthy
//!   and quiet, but still runs when the socket is down, when the delivery queue
//!   overflowed, when a frame could not be parsed, and on a periodic
//!   reconciliation regardless. Nothing is ever known *only* from the socket.
//!
//! `MEDULLA_INBOX_WS=0` disables the whole thing and restores pure polling.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::task::JoinHandle;

use ::tinyplace::types::MessageEnvelope;
use ::tinyplace::TinyPlaceClient;

/// How long to wait before re-opening a socket that closed or refused.
const REOPEN_DELAY: Duration = Duration::from_secs(5);

/// How many consecutive failed opens before giving up for good.
///
/// A backend that does not serve this endpoint would otherwise be reconnected
/// against forever. Polling still works, so giving up degrades latency rather
/// than function.
const MAX_CONSECUTIVE_FAILURES: u32 = 5;

/// How many delivered envelopes to hold before falling back to fetching.
///
/// Bounded because a drain loop that stalls must not let the queue grow without
/// limit. Overflow loses nothing: the envelopes are still in the mailbox until
/// they are acknowledged, so the fetch this forces picks them up.
const MAX_QUEUED: usize = 256;

/// How many recently handled message ids to remember for deduplication.
///
/// Comfortably more than the 50-envelope snapshot a reconnect replays, which is
/// the case it exists for.
const SEEN_CAPACITY: usize = 512;

/// How often to fetch even while the socket looks healthy.
///
/// Insurance, not the main path: it bounds how long an envelope could sit
/// unnoticed if the stream ever silently missed one.
pub const RECONCILE_INTERVAL: Duration = Duration::from_secs(30);

/// Whether the push listener should be started at all.
///
/// On by default; `MEDULLA_INBOX_WS=0` opts out, which is the escape hatch if
/// the socket ever misbehaves against a particular relay.
pub fn ws_inbox_enabled(env: &HashMap<String, String>) -> bool {
    !matches!(
        env.get("MEDULLA_INBOX_WS").map(|v| v.trim()),
        Some("0") | Some("false") | Some("off")
    )
}

/// Envelopes the relay pushed, waiting to be decrypted.
///
/// Cheap to clone; every clone shares one queue and one waiter.
#[derive(Clone, Default)]
pub struct PushInbox {
    notify: Arc<Notify>,
    listening: Arc<AtomicBool>,
    queued: Arc<Mutex<VecDeque<MessageEnvelope>>>,
    /// Set when the socket could not be trusted to have delivered everything —
    /// a full queue, an unparseable frame, or a closed stream.
    must_fetch: Arc<AtomicBool>,
}

impl PushInbox {
    /// An inbox nothing has been delivered to yet.
    pub fn new() -> Self {
        PushInbox::default()
    }

    /// Queue envelopes the socket delivered and wake any waiting drain.
    ///
    /// Beyond [`MAX_QUEUED`] the remainder is dropped and a fetch is forced
    /// instead — they are still in the mailbox, so nothing is lost by declining
    /// to hold them here.
    fn deliver(&self, envelopes: Vec<MessageEnvelope>) {
        if envelopes.is_empty() {
            return;
        }
        {
            let mut queued = self.queued.lock().expect("push inbox lock");
            for envelope in envelopes {
                if queued.len() >= MAX_QUEUED {
                    self.must_fetch.store(true, Ordering::Relaxed);
                    break;
                }
                queued.push_back(envelope);
            }
        }
        self.notify.notify_one();
    }

    /// Wake a waiting drain without delivering anything, and make it fetch.
    ///
    /// Used whenever the socket cannot vouch for what it carried: a frame that
    /// would not parse, or a stream that just closed.
    fn nudge(&self) {
        self.must_fetch.store(true, Ordering::Relaxed);
        self.notify.notify_one();
    }

    /// Take everything delivered so far.
    pub fn take(&self) -> Vec<MessageEnvelope> {
        self.queued
            .lock()
            .expect("push inbox lock")
            .drain(..)
            .collect()
    }

    /// Whether a fetch is owed, clearing the flag.
    pub fn take_must_fetch(&self) -> bool {
        self.must_fetch.swap(false, Ordering::Relaxed)
    }

    /// Whether a socket is currently open and delivering.
    ///
    /// Distinguishes "quiet because nothing is happening" from "quiet because
    /// the push channel is down and we are back to polling".
    pub fn is_listening(&self) -> bool {
        self.listening.load(Ordering::Relaxed)
    }

    fn set_listening(&self, value: bool) {
        self.listening.store(value, Ordering::Relaxed);
    }

    /// Wait for a delivery, or for `poll` to elapse — whichever comes first.
    ///
    /// The timeout is not a fallback that can be dropped once the socket works:
    /// it is the correctness floor.
    pub async fn wait(&self, poll: Duration) {
        tokio::select! {
            _ = self.notify.notified() => {}
            _ = tokio::time::sleep(poll) => {}
        }
    }
}

/// Message ids already handled, so a redelivery is not processed twice.
///
/// Bounded and insertion-ordered: the oldest id is forgotten once the set is
/// full, which is safe because a redelivery that old has long since been
/// acknowledged and will not be sent again.
#[derive(Clone, Default)]
pub struct SeenIds {
    order: Arc<Mutex<(VecDeque<String>, HashSet<String>)>>,
}

impl SeenIds {
    /// An empty set.
    pub fn new() -> Self {
        SeenIds::default()
    }

    /// Record `id`, reporting whether it is new.
    ///
    /// An empty id is always treated as new: the relay rejects those, so one
    /// appearing here is malformed rather than a duplicate, and collapsing all
    /// of them onto one another would drop real traffic.
    pub fn insert(&self, id: &str) -> bool {
        if id.is_empty() {
            return true;
        }
        let mut guard = self.order.lock().expect("seen ids lock");
        let (order, set) = &mut *guard;
        if !set.insert(id.to_string()) {
            return false;
        }
        order.push_back(id.to_string());
        while order.len() > SEEN_CAPACITY {
            if let Some(oldest) = order.pop_front() {
                set.remove(&oldest);
            }
        }
        true
    }
}

/// A running listener, stopped when this guard drops.
///
/// A bare [`JoinHandle`] would be wrong here: dropping one *detaches* the task
/// rather than aborting it, so a listener tied to a session's lifetime would
/// outlive it and hold a socket open for a drain loop that no longer exists.
pub struct ListenerGuard(JoinHandle<()>);

impl ListenerGuard {
    /// Stop the listener now rather than at drop.
    pub fn abort(&self) {
        self.0.abort();
    }
}

impl Drop for ListenerGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Pull the envelopes out of one stream frame.
///
/// Two shapes cross this socket: the snapshot the relay opens with, carrying the
/// mailbox as `data.messages`, and each live relay event, carrying one envelope
/// as `data`. Anything else — a keepalive, a frame kind added later — yields
/// nothing, and the caller treats that as a reason to fetch rather than as an
/// empty mailbox.
pub fn envelopes_in_frame(frame: &serde_json::Value) -> Option<Vec<MessageEnvelope>> {
    match frame.get("type").and_then(|t| t.as_str())? {
        "snapshot" => {
            let messages = frame.get("data")?.get("messages")?;
            serde_json::from_value::<Vec<MessageEnvelope>>(messages.clone()).ok()
        }
        "a2a.message" => {
            let envelope =
                serde_json::from_value::<MessageEnvelope>(frame.get("data")?.clone()).ok()?;
            Some(vec![envelope])
        }
        _ => None,
    }
}

/// Hold a push socket open for `agent_id`, delivering what it carries to `inbox`.
///
/// Returns immediately; the work happens on the guarded task, and dropping the
/// guard closes the socket. Errors are not surfaced — the drain loop's fetch is
/// unaffected by this failing, which is the point.
pub fn spawn_inbox_listener(
    client: TinyPlaceClient,
    agent_id: String,
    inbox: PushInbox,
    log: Option<super::LogFn>,
) -> ListenerGuard {
    ListenerGuard(tokio::spawn(async move {
        let mut failures = 0u32;
        loop {
            match client.a2a.stream(&agent_id).connect().await {
                Ok(mut connection) => {
                    failures = 0;
                    inbox.set_listening(true);
                    if let Some(log) = &log {
                        log("inbox: push channel open");
                    }
                    while let Some(frame) = connection.recv().await {
                        match frame.as_ref().ok().and_then(envelopes_in_frame) {
                            Some(envelopes) => inbox.deliver(envelopes),
                            // A frame we could not read might have carried mail.
                            // Fetching is the only honest response.
                            None => inbox.nudge(),
                        }
                    }
                    inbox.set_listening(false);
                    if let Some(log) = &log {
                        log("inbox: push channel closed — fetching until it reopens");
                    }
                }
                Err(error) => {
                    failures += 1;
                    if failures >= MAX_CONSECUTIVE_FAILURES {
                        // Say so once, with the reason. A silent fallback to
                        // fetching is indistinguishable from a relay that is
                        // simply quiet, and those want different fixes.
                        if let Some(log) = &log {
                            log(&format!(
                                "inbox: push channel unavailable after {failures} attempts ({error}) — fetching only"
                            ));
                        }
                        inbox.set_listening(false);
                        inbox.nudge();
                        return;
                    }
                }
            }
            // A socket that just dropped may have done so holding mail.
            inbox.nudge();
            tokio::time::sleep(REOPEN_DELAY).await;
        }
    }))
}

#[cfg(test)]
mod tests;
