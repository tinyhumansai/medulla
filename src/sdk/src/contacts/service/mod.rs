//! The relay side of contact management: poll incoming requests into a
//! [`ContactBook`], apply the admission policy, and perform operator decisions.
//!
//! The relay seam is a trait rather than a concrete client so the whole
//! admission flow is testable offline. No production relay ships here: the host
//! link has no contact graph to poll (protocol §7 — enrollment is the only
//! handshake), so what remains is the queue model the worker TUI renders.

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::task::JoinHandle;

use super::book::ContactBook;
use super::types::ContactDecision;

/// The relay operations contact management needs.
pub trait ContactRelay: Send + Sync {
    /// List pending incoming requests.
    fn incoming(&self) -> BoxFuture<'_, Result<Vec<IncomingRequest>, String>>;
    /// List the peers already accepted as contacts.
    ///
    /// Distinct from [`incoming`](Self::incoming), and not derivable from it: an
    /// accepted contact is no longer a pending request, so a book built only
    /// from arrivals knows about exactly the contacts this process happened to
    /// accept while it was running. That book is empty after a restart, and
    /// never contains a peer *this* agent requested and who accepted — both of
    /// which are real contacts that can dispatch work here.
    fn accepted(&self) -> BoxFuture<'_, Result<Vec<IncomingRequest>, String>>;
    /// Accept a request from `agent_id`.
    fn accept(&self, agent_id: String) -> BoxFuture<'_, Result<(), String>>;
    /// Decline an incoming request (or remove an existing contact).
    fn decline(&self, agent_id: String) -> BoxFuture<'_, Result<(), String>>;
    /// Block `agent_id`, refusing this and future requests.
    fn block(&self, agent_id: String) -> BoxFuture<'_, Result<(), String>>;
}

/// Poll `relay` for incoming requests, record them in `book`, and auto-settle
/// the ones policy admits.
///
/// One tick. Returns how many new requests were observed. Errors are swallowed
/// and retried on the next tick — a briefly unreachable relay is normal.
pub async fn poll_once(
    relay: &dyn ContactRelay,
    book: &ContactBook,
    now: &NowFn,
) -> Result<usize, String> {
    // Reconcile the established contacts first. They are not pending requests,
    // so nothing else in this loop would ever learn about them — which is why
    // the Contacts tab used to be empty on a fresh start no matter how many
    // peers the relay knew about.
    reconcile_contacts(relay, book, now).await?;

    let incoming = relay.incoming().await?;
    let mut observed = 0usize;
    for request in incoming {
        if book.observe(&request.agent_id, request.handle.clone(), now()) {
            observed += 1;
        }
        // A request the operator already settled must not be resurrected by a
        // later policy widening, so only two states are candidates for
        // auto-admission: one still pending, and one whose *own* automatic
        // attempt the relay refused. The second matters where nobody is watching
        // — the hub identity has no operator screen — because a single transient
        // `accept` failure would otherwise strand that peer until a restart. A
        // failed *operator* decision stays theirs to repeat.
        let retryable = book
            .get(&request.agent_id)
            .map(|record| match record.state {
                super::types::RequestState::Pending => true,
                super::types::RequestState::Failed => record.auto,
                _ => false,
            })
            .unwrap_or(false);
        if !retryable {
            continue;
        }
        if let Some(decision) = book.auto_decision(&request.agent_id) {
            let _ = decide(relay, book, &request.agent_id, decision, true, now).await;
        }
    }
    Ok(observed)
}

/// Re-read the relay's contact list into `book`, returning how many were new.
///
/// Additive on purpose: a peer the relay no longer lists is **not** demoted
/// here. `list` is a paginated endpoint, so a truncated page would otherwise
/// silently strip real contacts — and losing a contact is far worse than
/// carrying a stale one, because a contact is what admits a peer's work. A
/// removal made *here* is already reflected the moment it settles.
pub async fn reconcile_contacts(
    relay: &dyn ContactRelay,
    book: &ContactBook,
    now: &NowFn,
) -> Result<usize, String> {
    let mut added = 0usize;
    for contact in relay.accepted().await? {
        if book.record_contact(&contact.agent_id, contact.handle.clone(), now()) {
            added += 1;
        }
    }
    Ok(added)
}

/// Perform one decision against the relay and record the result.
///
/// `auto` marks the decision as policy-driven rather than operator-driven, which
/// the UI surfaces so an operator can tell what they approved from what the
/// policy did on their behalf.
pub async fn decide(
    relay: &dyn ContactRelay,
    book: &ContactBook,
    agent_id: &str,
    decision: ContactDecision,
    auto: bool,
    now: &NowFn,
) -> Result<(), String> {
    if !book.begin(agent_id, now()) {
        return Err(format!("no actionable contact request for {agent_id}"));
    }
    let result = match decision {
        ContactDecision::Accept => relay.accept(agent_id.to_string()).await,
        ContactDecision::Decline => relay.decline(agent_id.to_string()).await,
        ContactDecision::Block => relay.block(agent_id.to_string()).await,
    };
    match result {
        Ok(()) => {
            book.settle(agent_id, decision, auto, now());
            Ok(())
        }
        Err(message) => {
            book.fail(agent_id, message.clone(), auto, now());
            Err(message)
        }
    }
}

/// Spawn a loop that polls `relay` every `interval`, filling `book`.
///
/// Best-effort: transient errors are ignored and retried. The loop ends when the
/// returned handle is aborted.
pub fn spawn_contact_poll(
    relay: Arc<dyn ContactRelay>,
    book: ContactBook,
    interval: Duration,
    now: NowFn,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let _ = poll_once(relay.as_ref(), &book, &now).await;
            tokio::time::sleep(interval).await;
        }
    })
}

mod types;
pub use types::IncomingRequest;
pub use types::NowFn;
