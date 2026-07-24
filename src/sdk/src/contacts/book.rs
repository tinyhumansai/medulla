//! [`ContactBook`] — the pending-request queue the operator works through, and
//! the policy that decides which requests never reach them.
//!
//! Pure state: the book records what the relay reported and what was decided,
//! and answers "what should happen to this request". Performing the decision
//! against the relay is [`super::service`]'s job, so the whole admission model
//! is testable without a network.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use super::types::{AdmissionPolicy, ContactDecision, ContactRequest, RequestState};

/// The observed pending queue plus the operator's decisions.
///
/// Cheap to clone (an `Arc`), so the poll loop and the UI share one.
#[derive(Clone)]
pub struct ContactBook {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    policy: AdmissionPolicy,
    allowlist: HashSet<String>,
    /// Requests in first-seen order, so the list does not reshuffle under the
    /// operator's cursor as the poll loop runs.
    requests: Vec<ContactRequest>,
}

impl ContactBook {
    /// Build a book with `policy` and an allowlist of peer cryptoIds.
    pub fn new(policy: AdmissionPolicy, allowlist: impl IntoIterator<Item = String>) -> Self {
        ContactBook {
            inner: Arc::new(Mutex::new(Inner {
                policy,
                allowlist: allowlist.into_iter().collect(),
                requests: Vec::new(),
            })),
        }
    }

    /// The active admission policy.
    pub fn policy(&self) -> AdmissionPolicy {
        self.inner.lock().unwrap().policy
    }

    /// Change the admission policy.
    ///
    /// Widening the policy does **not** retroactively accept requests already
    /// declined or blocked — a reversal must be deliberate, so those stay put
    /// and only [`RequestState::Pending`] ones become auto-admissible.
    pub fn set_policy(&self, policy: AdmissionPolicy) {
        self.inner.lock().unwrap().policy = policy;
    }

    /// Add a peer to the allowlist.
    pub fn allow(&self, agent_id: impl Into<String>) {
        self.inner.lock().unwrap().allowlist.insert(agent_id.into());
    }

    /// Whether `agent_id` is on the allowlist.
    pub fn is_allowed(&self, agent_id: &str) -> bool {
        self.inner.lock().unwrap().allowlist.contains(agent_id)
    }

    /// Every known request, first-seen order.
    pub fn requests(&self) -> Vec<ContactRequest> {
        self.inner.lock().unwrap().requests.clone()
    }

    /// Only the requests still waiting on a decision.
    pub fn pending(&self) -> Vec<ContactRequest> {
        self.inner
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|request| request.state == RequestState::Pending)
            .cloned()
            .collect()
    }

    /// How many requests are waiting on the operator — the badge count.
    pub fn pending_count(&self) -> usize {
        self.inner
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|request| request.state == RequestState::Pending)
            .count()
    }

    /// One request by peer id.
    pub fn get(&self, agent_id: &str) -> Option<ContactRequest> {
        self.inner
            .lock()
            .unwrap()
            .requests
            .iter()
            .find(|request| request.agent_id == agent_id)
            .cloned()
    }

    /// Record an incoming request the relay reported.
    ///
    /// Idempotent: re-observing a request the operator already settled must not
    /// resurrect it as pending, because the relay keeps reporting a declined
    /// request until the peer gives up. Only `handle` is refreshed for a known
    /// peer.
    ///
    /// Returns whether this was a new request.
    pub fn observe(&self, agent_id: &str, handle: Option<String>, now: i64) -> bool {
        if agent_id.trim().is_empty() {
            return false;
        }
        let mut inner = self.inner.lock().unwrap();
        if let Some(existing) = inner
            .requests
            .iter_mut()
            .find(|request| request.agent_id == agent_id)
        {
            if handle.is_some() && existing.handle != handle {
                existing.handle = handle;
            }
            return false;
        }
        inner.requests.push(ContactRequest {
            agent_id: agent_id.to_string(),
            handle,
            state: RequestState::Pending,
            first_seen_at: now,
            updated_at: now,
            last_error: None,
            auto: false,
        });
        true
    }

    /// What policy says should happen to `agent_id` without operator input.
    ///
    /// `None` means "ask the operator".
    pub fn auto_decision(&self, agent_id: &str) -> Option<ContactDecision> {
        let inner = self.inner.lock().unwrap();
        match inner.policy {
            AdmissionPolicy::All => Some(ContactDecision::Accept),
            AdmissionPolicy::Allowlist if inner.allowlist.contains(agent_id) => {
                Some(ContactDecision::Accept)
            }
            // Under `allowlist`, a peer that is not listed is *queued*, not
            // declined: the operator may still want to admit it, and declining
            // on their behalf would hide the request entirely.
            AdmissionPolicy::Allowlist | AdmissionPolicy::Manual => None,
        }
    }

    /// Mark a decision as in flight, so the UI shows it and a second keypress
    /// does not double-submit.
    ///
    /// Returns `false` when the request is unknown or not actionable.
    pub fn begin(&self, agent_id: &str, now: i64) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let Some(request) = inner
            .requests
            .iter_mut()
            .find(|request| request.agent_id == agent_id)
        else {
            return false;
        };
        if !request.state.is_actionable() {
            return false;
        }
        request.state = RequestState::Accepting;
        request.updated_at = now;
        request.last_error = None;
        true
    }

    /// Record a peer the relay reports as an established contact.
    ///
    /// The relay is authoritative about the contact graph, so this overrides
    /// whatever the book believed: a peer accepted from another device, or
    /// before this process started, is a contact here too. Returns whether the
    /// book changed, so a caller can narrate only real news.
    ///
    /// Deliberately not `observe` + `settle`: `observe` inserts as *pending*,
    /// which would flash the peer through the Requests tab and, under an
    /// auto-admission policy, spend a decision on a relationship that already
    /// exists.
    pub fn record_contact(&self, agent_id: &str, handle: Option<String>, now: i64) -> bool {
        if agent_id.trim().is_empty() {
            return false;
        }
        let mut inner = self.inner.lock().unwrap();
        if let Some(existing) = inner
            .requests
            .iter_mut()
            .find(|request| request.agent_id == agent_id)
        {
            if handle.is_some() && existing.handle != handle {
                existing.handle = handle;
            }
            if existing.state == RequestState::Accepted {
                return false;
            }
            existing.state = RequestState::Accepted;
            existing.updated_at = now;
            existing.last_error = None;
            return true;
        }
        inner.requests.push(ContactRequest {
            agent_id: agent_id.to_string(),
            handle,
            state: RequestState::Accepted,
            first_seen_at: now,
            updated_at: now,
            last_error: None,
            // Not a decision this daemon made in this run — it is the relay's
            // existing state, so crediting policy for it would be a fiction.
            auto: false,
        });
        true
    }

    /// Record a decision that succeeded against the relay.
    pub fn settle(&self, agent_id: &str, decision: ContactDecision, auto: bool, now: i64) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(request) = inner
            .requests
            .iter_mut()
            .find(|request| request.agent_id == agent_id)
        {
            request.state = decision.settled_state();
            request.updated_at = now;
            request.last_error = None;
            request.auto = auto;
        }
    }

    /// Record a decision that failed, leaving it retryable.
    pub fn fail(&self, agent_id: &str, message: impl Into<String>, now: i64) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(request) = inner
            .requests
            .iter_mut()
            .find(|request| request.agent_id == agent_id)
        {
            request.state = RequestState::Failed;
            request.updated_at = now;
            request.last_error = Some(message.into());
        }
    }

    /// Whether a peer has been accepted, and may therefore dispatch work here.
    pub fn is_accepted(&self, agent_id: &str) -> bool {
        self.inner
            .lock()
            .unwrap()
            .requests
            .iter()
            .any(|request| request.agent_id == agent_id && request.state == RequestState::Accepted)
    }

    /// Drop settled requests older than `before`, keeping the list from growing
    /// without bound on a long-lived daemon. Pending ones are never pruned.
    ///
    /// Returns how many were dropped.
    pub fn prune_settled(&self, before: i64) -> usize {
        let mut inner = self.inner.lock().unwrap();
        let len = inner.requests.len();
        inner.requests.retain(|request| {
            request.state == RequestState::Pending || request.updated_at >= before
        });
        len - inner.requests.len()
    }
}

impl Default for ContactBook {
    fn default() -> Self {
        ContactBook::new(AdmissionPolicy::default(), Vec::new())
    }
}
