//! Data model for incoming contact-request management: the admission policy, a
//! pending request, and the operator's decision.
//!
//! Contact requests matter because the relay refuses a DM between two agents
//! that are not accepted contacts. Accepting one is therefore not a nicety — it
//! is the act that lets a peer send this daemon work. Every prior
//! implementation auto-accepted; this module makes that a *choice*.

use serde::{Deserialize, Serialize};

/// How incoming contact requests are admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdmissionPolicy {
    /// Accept nothing automatically; every request waits for the operator.
    ///
    /// The safe default: an accepted contact can dispatch work to this machine's
    /// coding agents, so admitting one is a privilege grant.
    #[default]
    Manual,
    /// Accept requests from peers on the configured allowlist; queue the rest.
    Allowlist,
    /// Accept everything. Appropriate only on a daemon that is deliberately open.
    All,
}

impl AdmissionPolicy {
    /// The wire/display string.
    pub fn as_str(self) -> &'static str {
        match self {
            AdmissionPolicy::Manual => "manual",
            AdmissionPolicy::Allowlist => "allowlist",
            AdmissionPolicy::All => "all",
        }
    }

    /// Parse a policy name.
    ///
    /// Unknown values fall back to [`AdmissionPolicy::Manual`] — the closed
    /// direction. A typo in configuration must never widen access.
    pub fn parse(value: &str) -> AdmissionPolicy {
        match value.trim().to_ascii_lowercase().as_str() {
            "all" | "any" => AdmissionPolicy::All,
            "allowlist" | "peers" | "configured" => AdmissionPolicy::Allowlist,
            _ => AdmissionPolicy::Manual,
        }
    }
}

/// Where a pending request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestState {
    /// Waiting on the operator.
    Pending,
    /// The operator (or policy) accepted it; the accept call is in flight.
    Accepting,
    /// Accepted — this peer may now DM us.
    Accepted,
    /// Declined; the relationship was removed.
    Declined,
    /// Blocked; further requests from this peer are refused.
    Blocked,
    /// A decision failed against the relay and can be retried.
    Failed,
}

impl RequestState {
    /// The display string.
    pub fn as_str(self) -> &'static str {
        match self {
            RequestState::Pending => "pending",
            RequestState::Accepting => "accepting",
            RequestState::Accepted => "accepted",
            RequestState::Declined => "declined",
            RequestState::Blocked => "blocked",
            RequestState::Failed => "failed",
        }
    }

    /// A single-width glyph for dense list rendering.
    pub fn glyph(self) -> char {
        match self {
            RequestState::Pending => '?',
            RequestState::Accepting => '◌',
            RequestState::Accepted => '✓',
            RequestState::Declined => '–',
            RequestState::Blocked => '⊘',
            RequestState::Failed => '✕',
        }
    }

    /// Whether the operator can still act on a request in this state.
    ///
    /// A failed decision is actionable again — the relay may have been briefly
    /// unreachable.
    pub fn is_actionable(self) -> bool {
        matches!(self, RequestState::Pending | RequestState::Failed)
    }
}

/// One incoming contact request awaiting (or having received) a decision.
#[derive(Debug, Clone, PartialEq)]
pub struct ContactRequest {
    /// The requesting peer's cryptoId. This is the identity that will appear as
    /// the authenticated sender on every frame it later dispatches, so it is the
    /// thing the operator is actually approving.
    pub agent_id: String,
    /// The peer's directory handle, when the relay reported one.
    pub handle: Option<String>,
    /// Where the request stands.
    pub state: RequestState,
    /// Epoch ms when this request was first observed.
    pub first_seen_at: i64,
    /// Epoch ms of the most recent state change.
    pub updated_at: i64,
    /// Why the last decision failed, when it did.
    pub last_error: Option<String>,
    /// Whether policy (rather than the operator) settled this request.
    pub auto: bool,
}

impl ContactRequest {
    /// The label to show the operator: the handle when known, else the id.
    pub fn display_name(&self) -> &str {
        self.handle.as_deref().unwrap_or(&self.agent_id)
    }
}

/// A decision the operator (or policy) makes about a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactDecision {
    /// Accept the request; the peer may then DM this agent.
    Accept,
    /// Decline it, removing the relationship. The peer may request again.
    Decline,
    /// Block the peer, refusing this and future requests.
    Block,
}

impl ContactDecision {
    /// The display string.
    pub fn as_str(self) -> &'static str {
        match self {
            ContactDecision::Accept => "accept",
            ContactDecision::Decline => "decline",
            ContactDecision::Block => "block",
        }
    }

    /// The state a request lands in once this decision succeeds.
    pub fn settled_state(self) -> RequestState {
        match self {
            ContactDecision::Accept => RequestState::Accepted,
            ContactDecision::Decline => RequestState::Declined,
            ContactDecision::Block => RequestState::Blocked,
        }
    }
}
