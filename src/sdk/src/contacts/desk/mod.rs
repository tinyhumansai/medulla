//! [`ContactDesk`] — the book, the relay, and the clock bundled into the one
//! handle a UI holds.
//!
//! The UI should not have to thread three collaborators through every keypress,
//! and it should never talk to the relay directly. The desk is that seam: read
//! the queue synchronously for rendering, dispatch a decision asynchronously.

use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;

use std::sync::Mutex;

use super::book::ContactBook;
use super::service::{decide, poll_once, reconcile_contacts, ContactRelay, NowFn};
use super::types::{AdmissionPolicy, ContactDecision, ContactRequest};

impl PollHealth {
    /// A short line for the UI.
    pub fn summary(&self, now: i64) -> String {
        match self {
            PollHealth::Pending => "not polled yet".to_string(),
            PollHealth::Ok { at, seen } => {
                let ago = now.saturating_sub(*at).max(0) / 1_000;
                format!("checked {ago}s ago · {seen} seen")
            }
            PollHealth::Failed { error, .. } => format!("relay unreachable: {error}"),
        }
    }

    /// Whether the last attempt failed.
    pub fn is_failing(&self) -> bool {
        matches!(self, PollHealth::Failed { .. })
    }
}

impl ContactDesk {
    /// Build a desk over `relay` with `policy` and an allowlist of peer
    /// cryptoIds.
    pub fn new(
        relay: Arc<dyn ContactRelay>,
        policy: AdmissionPolicy,
        allowlist: impl IntoIterator<Item = String>,
    ) -> Self {
        ContactDesk {
            book: ContactBook::new(policy, allowlist),
            relay,
            now: Arc::new(crate::clock::now_millis),
            health: Arc::new(Mutex::new(PollHealth::Pending)),
            log: Arc::new(Mutex::new(None)),
        }
    }

    /// Narrate contact activity to `log`.
    ///
    /// Takes effect for every handle of this desk, including one already
    /// polling — so the caller that wants the narration does not have to be the
    /// caller that owns the poll.
    pub fn with_log(self, log: crate::logging::LineSink) -> Self {
        *self.log.lock().expect("contact log lock") = Some(log);
        self
    }

    /// Emit a line if a sink is attached.
    fn say(&self, line: &str) {
        // Clone the sink out before calling it: the sink is arbitrary caller
        // code, and holding the lock across it invites a deadlock.
        let sink = self.log.lock().expect("contact log lock").clone();
        if let Some(log) = sink {
            log(line);
        }
    }

    /// How the most recent poll of the relay went.
    pub fn health(&self) -> PollHealth {
        self.health.lock().unwrap().clone()
    }

    /// Poll the relay once, recording the outcome.
    ///
    /// Also the operator's manual refresh: waiting out a background interval to
    /// learn whether anything is arriving is not much of an answer.
    pub async fn refresh(&self) -> PollHealth {
        let before_pending = self.book.pending_count();
        let before_contacts: std::collections::HashSet<String> = self
            .accepted()
            .into_iter()
            .map(|contact| contact.agent_id)
            .collect();
        let outcome = poll_once(self.relay.as_ref(), &self.book, &self.now).await;
        let at = (self.now)();
        let health = match outcome {
            Ok(_) => {
                let seen = self.book.requests().len();
                // Only new arrivals are worth a line; a poll that finds the same
                // queue every 1.5s would drown the log it is meant to explain.
                // Requests and contacts are counted apart: a peer that shows up
                // already accepted is not something waiting on the operator, and
                // reporting it as a pending request would send them to a tab
                // with nothing in it.
                let pending = self.book.pending_count();
                if pending > before_pending {
                    self.say(&format!(
                        "contacts: {} new request(s) — {pending} now pending",
                        pending - before_pending,
                    ));
                }
                let established = self.accepted();
                // Named, not just counted: a peer that pairs itself (a worker
                // naming this identity as its master) settles without anyone
                // deciding anything here, and "1 new contact" gives an operator
                // nothing to check against who they expected.
                let fresh: Vec<String> = established
                    .iter()
                    .map(|contact| contact.agent_id.clone())
                    .filter(|id| !before_contacts.contains(id))
                    .collect();
                if !fresh.is_empty() {
                    self.say(&format!(
                        "contacts: {} contact(s) established — {} ({} total)",
                        fresh.len(),
                        fresh.join(", "),
                        established.len(),
                    ));
                }
                PollHealth::Ok { at, seen }
            }
            Err(error) => {
                // Failures are rate-limited by only speaking on a change of
                // state, so a persistently dead relay says so once.
                if !self.health.lock().unwrap().is_failing() {
                    self.say(&format!("contacts: relay unreachable — {error}"));
                }
                PollHealth::Failed { at, error }
            }
        };
        *self.health.lock().unwrap() = health.clone();
        health
    }

    /// Override the clock (tests).
    pub fn with_now(mut self, now: NowFn) -> Self {
        self.now = now;
        self
    }

    /// The underlying queue, for reads the UI does while rendering.
    pub fn book(&self) -> &ContactBook {
        &self.book
    }

    /// Every known request, first-seen order.
    pub fn requests(&self) -> Vec<ContactRequest> {
        self.book.requests()
    }

    /// How many requests are waiting on the operator — the tab badge count.
    pub fn pending_count(&self) -> usize {
        self.book.pending_count()
    }

    /// The established contacts — peers that may dispatch work here.
    ///
    /// Reconciled from the relay on every poll, so this is the contact graph as
    /// the relay sees it rather than only the requests this process accepted
    /// while it happened to be running.
    pub fn accepted(&self) -> Vec<ContactRequest> {
        self.book
            .requests()
            .into_iter()
            .filter(|request| request.state == super::types::RequestState::Accepted)
            .collect()
    }

    /// The active admission policy.
    pub fn policy(&self) -> AdmissionPolicy {
        self.book.policy()
    }

    /// Admit `agent_id` under the `allowlist` policy from now on.
    ///
    /// The allowlist is seeded once, at construction, from the peers the config
    /// named — so a peer paired *during* the session was not on it, and its
    /// request queued as though nobody had paired anything. Any pairing made at
    /// runtime has to say so here, or the operator's decision only takes effect
    /// after a restart. Already-settled requests are untouched: widening
    /// admission never resurrects a declined peer (see
    /// [`ContactBook::set_policy`](super::book::ContactBook::set_policy)).
    pub fn allow(&self, agent_id: impl Into<String>) {
        self.book.allow(agent_id);
    }

    /// Cycle the admission policy, returning the new one.
    ///
    /// Cycles `manual → allowlist → all → manual`, so the closed setting is
    /// always one step away.
    pub fn cycle_policy(&self) -> AdmissionPolicy {
        let next = match self.book.policy() {
            AdmissionPolicy::Manual => AdmissionPolicy::Allowlist,
            AdmissionPolicy::Allowlist => AdmissionPolicy::All,
            AdmissionPolicy::All => AdmissionPolicy::Manual,
        };
        self.book.set_policy(next);
        next
    }

    /// Perform one operator decision, returning the status line to show.
    ///
    /// On success the relay's contact list is re-read straight away rather than
    /// waiting out the poll interval, so the Contacts tab reflects what the
    /// relay actually recorded rather than this daemon's expectation of it.
    pub async fn decide(
        &self,
        agent_id: &str,
        decision: ContactDecision,
    ) -> Result<String, String> {
        decide(
            self.relay.as_ref(),
            &self.book,
            agent_id,
            decision,
            false,
            &self.now,
        )
        .await
        .map_err(|message| format!("{agent_id} · {message}"))?;

        // Best-effort: the decision already succeeded, so a failed re-read must
        // not report it as a failure. The background poll will catch up.
        let _ = reconcile_contacts(self.relay.as_ref(), &self.book, &self.now).await;

        let line = match decision {
            ContactDecision::Accept => {
                format!("{agent_id} accepted — it may now dispatch work here")
            }
            ContactDecision::Decline => format!("{agent_id} declined"),
            ContactDecision::Block => format!("{agent_id} blocked"),
        };
        self.say(&format!("contacts: {line}"));
        Ok(line)
    }

    /// Start the background poll that keeps the queue current.
    ///
    /// Every tick records its outcome, so a relay that is failing shows as
    /// failing rather than as an empty queue.
    pub fn spawn_poll(&self, interval: Duration) -> JoinHandle<()> {
        let desk = self.clone();
        tokio::spawn(async move {
            loop {
                desk.refresh().await;
                tokio::time::sleep(interval).await;
            }
        })
    }
}

mod types;
pub use types::ContactDesk;
pub use types::PollHealth;
