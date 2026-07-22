//! Unit tests for contact-request admission: the policy evaluation, the
//! idempotent pending queue, and decision execution against a fake relay.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;

use super::book::ContactBook;
use super::desk::ContactDesk;
use super::service::{decide, poll_once, ContactRelay, IncomingRequest, NowFn};
use super::types::{AdmissionPolicy, ContactDecision, RequestState};

/// A clock that advances one step per read.
fn clock() -> NowFn {
    let counter = Arc::new(AtomicI64::new(100));
    Arc::new(move || counter.fetch_add(1, Ordering::SeqCst))
}

/// A relay that serves a fixed incoming list and records every call.
#[derive(Default)]
struct FakeRelay {
    incoming: Mutex<Vec<IncomingRequest>>,
    /// Peers the relay already considers contacts.
    accepted: Mutex<Vec<IncomingRequest>>,
    calls: Mutex<Vec<String>>,
    fail: Mutex<bool>,
    /// Fails only the contact-list read, so a decision can succeed while the
    /// re-read that follows it does not.
    fail_list: Mutex<bool>,
    /// Every relay interaction in order, listings included.
    trace: Mutex<Vec<String>>,
}

impl FakeRelay {
    fn with_incoming(ids: &[&str]) -> Arc<Self> {
        Arc::new(FakeRelay {
            incoming: Mutex::new(
                ids.iter()
                    .map(|id| IncomingRequest {
                        agent_id: (*id).to_string(),
                        handle: None,
                    })
                    .collect(),
            ),
            ..FakeRelay::default()
        })
    }

    /// A relay whose contact graph already holds `ids` — peers accepted before
    /// this process started, or from another device.
    fn with_contacts(ids: &[&str]) -> Arc<Self> {
        Arc::new(FakeRelay {
            accepted: Mutex::new(
                ids.iter()
                    .map(|id| IncomingRequest {
                        agent_id: (*id).to_string(),
                        handle: None,
                    })
                    .collect(),
            ),
            ..FakeRelay::default()
        })
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn trace(&self) -> Vec<String> {
        self.trace.lock().unwrap().clone()
    }

    fn record(&self, call: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(call.to_string());
        self.trace.lock().unwrap().push(call.to_string());
        if *self.fail.lock().unwrap() {
            return Err("relay unreachable".to_string());
        }
        Ok(())
    }
}

impl ContactRelay for FakeRelay {
    fn incoming(&self) -> BoxFuture<'_, Result<Vec<IncomingRequest>, String>> {
        Box::pin(async move {
            // A relay that is down cannot list either, so the fail flag covers
            // listing as well as decisions.
            if *self.fail.lock().unwrap() {
                return Err("relay unreachable".to_string());
            }
            Ok(self.incoming.lock().unwrap().clone())
        })
    }
    fn accepted(&self) -> BoxFuture<'_, Result<Vec<IncomingRequest>, String>> {
        Box::pin(async move {
            // Traced (not recorded as a "call") so a test can tell "the list
            // was re-read" apart from "the local settle happened to leave the
            // right answer", without disturbing the decision-only assertions.
            self.trace.lock().unwrap().push("list".to_string());
            if *self.fail.lock().unwrap() || *self.fail_list.lock().unwrap() {
                return Err("relay unreachable".to_string());
            }
            Ok(self.accepted.lock().unwrap().clone())
        })
    }
    fn accept(&self, agent_id: String) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            self.record(&format!("accept:{agent_id}"))?;
            // Mirror the relay: accepting moves the peer out of the request
            // queue and into the contact list.
            self.incoming
                .lock()
                .unwrap()
                .retain(|request| request.agent_id != agent_id);
            self.accepted.lock().unwrap().push(IncomingRequest {
                agent_id,
                handle: None,
            });
            Ok(())
        })
    }
    fn decline(&self, agent_id: String) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move { self.record(&format!("decline:{agent_id}")) })
    }
    fn block(&self, agent_id: String) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move { self.record(&format!("block:{agent_id}")) })
    }
}

// ----------------------------------------------------------------- policy ---

#[test]
fn unknown_policy_names_close_rather_than_open() {
    // A typo in configuration must never widen access.
    assert_eq!(AdmissionPolicy::parse("nonsense"), AdmissionPolicy::Manual);
    assert_eq!(AdmissionPolicy::parse(""), AdmissionPolicy::Manual);
    assert_eq!(AdmissionPolicy::parse("all"), AdmissionPolicy::All);
    assert_eq!(
        AdmissionPolicy::parse("allowlist"),
        AdmissionPolicy::Allowlist
    );
}

#[test]
fn the_default_policy_admits_nothing_automatically() {
    let book = ContactBook::default();
    assert_eq!(book.policy(), AdmissionPolicy::Manual);
    book.observe("peer", None, 1);
    assert_eq!(book.auto_decision("peer"), None);
}

#[test]
fn allowlist_admits_the_listed_and_queues_the_rest() {
    let book = ContactBook::new(AdmissionPolicy::Allowlist, ["known".to_string()]);
    assert_eq!(book.auto_decision("known"), Some(ContactDecision::Accept));
    // Unlisted peers are queued, not declined: declining on the operator's
    // behalf would hide the request entirely.
    assert_eq!(book.auto_decision("stranger"), None);
}

// ------------------------------------------------------------------- book ---

#[test]
fn observing_the_same_request_twice_reports_it_once() {
    let book = ContactBook::default();
    assert!(book.observe("peer", None, 1), "first sight is new");
    assert!(!book.observe("peer", None, 2), "a repeat is not new");
    assert_eq!(book.requests().len(), 1);
    assert_eq!(book.pending_count(), 1);
}

#[test]
fn a_settled_request_is_never_resurrected_by_a_repeat_poll() {
    // The relay keeps reporting a declined request until the peer gives up, so
    // re-observation must not flip it back to pending.
    let book = ContactBook::default();
    book.observe("peer", None, 1);
    book.settle("peer", ContactDecision::Decline, false, 2);

    book.observe("peer", None, 3);
    assert_eq!(book.get("peer").unwrap().state, RequestState::Declined);
    assert_eq!(book.pending_count(), 0);
}

#[test]
fn a_handle_learned_later_refreshes_an_existing_request() {
    let book = ContactBook::default();
    book.observe("peer", None, 1);
    book.observe("peer", Some("@alice".to_string()), 2);
    assert_eq!(book.get("peer").unwrap().handle.as_deref(), Some("@alice"));
    assert_eq!(book.get("peer").unwrap().display_name(), "@alice");
}

#[test]
fn only_actionable_requests_can_begin_a_decision() {
    let book = ContactBook::default();
    book.observe("peer", None, 1);
    assert!(book.begin("peer", 2), "pending is actionable");
    assert!(!book.begin("peer", 3), "a decision is already in flight");

    book.fail("peer", "relay unreachable", 4);
    assert!(book.begin("peer", 5), "a failed decision is retryable");
    assert!(!book.begin("ghost", 6), "an unknown peer is not actionable");
}

#[test]
fn pruning_keeps_pending_requests_and_drops_old_settled_ones() {
    let book = ContactBook::default();
    book.observe("old", None, 1);
    book.observe("waiting", None, 1);
    book.settle("old", ContactDecision::Accept, false, 10);

    assert_eq!(book.prune_settled(50), 1);
    assert!(book.get("old").is_none());
    assert!(book.get("waiting").is_some(), "pending is never pruned");
}

#[test]
fn accepted_peers_are_reported_as_such() {
    let book = ContactBook::default();
    book.observe("peer", None, 1);
    assert!(!book.is_accepted("peer"));
    book.settle("peer", ContactDecision::Accept, false, 2);
    assert!(book.is_accepted("peer"));
}

// ---------------------------------------------------------------- service ---

#[tokio::test]
async fn manual_policy_queues_every_request_without_touching_the_relay() {
    let relay = FakeRelay::with_incoming(&["alice", "bob"]);
    let book = ContactBook::default();
    let now = clock();

    assert_eq!(poll_once(relay.as_ref(), &book, &now).await.unwrap(), 2);
    assert_eq!(book.pending_count(), 2);
    assert!(
        relay.calls().is_empty(),
        "manual admission must not accept anything on its own"
    );
}

#[tokio::test]
async fn allowlist_policy_auto_accepts_only_the_listed_peer() {
    let relay = FakeRelay::with_incoming(&["alice", "stranger"]);
    let book = ContactBook::new(AdmissionPolicy::Allowlist, ["alice".to_string()]);
    let now = clock();

    poll_once(relay.as_ref(), &book, &now).await.unwrap();

    assert_eq!(relay.calls(), vec!["accept:alice".to_string()]);
    assert_eq!(book.get("alice").unwrap().state, RequestState::Accepted);
    assert!(book.get("alice").unwrap().auto, "policy settled this one");
    assert_eq!(book.get("stranger").unwrap().state, RequestState::Pending);
}

#[tokio::test]
async fn a_second_poll_does_not_re_accept_an_already_settled_peer() {
    let relay = FakeRelay::with_incoming(&["alice"]);
    let book = ContactBook::new(AdmissionPolicy::All, Vec::<String>::new());
    let now = clock();

    poll_once(relay.as_ref(), &book, &now).await.unwrap();
    poll_once(relay.as_ref(), &book, &now).await.unwrap();

    assert_eq!(
        relay.calls().len(),
        1,
        "the relay keeps listing it; we must not keep accepting it"
    );
}

#[tokio::test]
async fn widening_the_policy_does_not_resurrect_a_declined_request() {
    // A reversal must be deliberate. This is the test that keeps an operator's
    // explicit "no" from being undone by a later config change.
    let relay = FakeRelay::with_incoming(&["stranger"]);
    let book = ContactBook::default();
    let now = clock();

    poll_once(relay.as_ref(), &book, &now).await.unwrap();
    decide(
        relay.as_ref(),
        &book,
        "stranger",
        ContactDecision::Decline,
        false,
        &now,
    )
    .await
    .unwrap();

    book.set_policy(AdmissionPolicy::All);
    poll_once(relay.as_ref(), &book, &now).await.unwrap();

    assert_eq!(book.get("stranger").unwrap().state, RequestState::Declined);
    assert_eq!(relay.calls(), vec!["decline:stranger".to_string()]);
}

#[tokio::test]
async fn an_operator_decision_records_the_outcome() {
    let relay = FakeRelay::with_incoming(&["alice"]);
    let book = ContactBook::default();
    let now = clock();
    poll_once(relay.as_ref(), &book, &now).await.unwrap();

    decide(
        relay.as_ref(),
        &book,
        "alice",
        ContactDecision::Accept,
        false,
        &now,
    )
    .await
    .unwrap();

    let request = book.get("alice").unwrap();
    assert_eq!(request.state, RequestState::Accepted);
    assert!(!request.auto, "the operator settled this one, not policy");
}

#[tokio::test]
async fn a_failed_decision_stays_retryable() {
    let relay = FakeRelay::with_incoming(&["alice"]);
    let book = ContactBook::default();
    let now = clock();
    poll_once(relay.as_ref(), &book, &now).await.unwrap();
    *relay.fail.lock().unwrap() = true;

    let outcome = decide(
        relay.as_ref(),
        &book,
        "alice",
        ContactDecision::Accept,
        false,
        &now,
    )
    .await;
    assert!(outcome.is_err());

    let request = book.get("alice").unwrap();
    assert_eq!(request.state, RequestState::Failed);
    assert!(request.state.is_actionable(), "the operator can retry");
    assert_eq!(request.last_error.as_deref(), Some("relay unreachable"));
}

#[tokio::test]
async fn deciding_on_an_unknown_peer_fails_loudly() {
    let relay = FakeRelay::with_incoming(&[]);
    let book = ContactBook::default();
    let now = clock();

    let outcome = decide(
        relay.as_ref(),
        &book,
        "ghost",
        ContactDecision::Accept,
        false,
        &now,
    )
    .await;
    assert!(outcome.is_err());
    assert!(relay.calls().is_empty());
}

#[tokio::test]
async fn blocking_a_peer_settles_it_as_blocked() {
    let relay = FakeRelay::with_incoming(&["spammer"]);
    let book = ContactBook::default();
    let now = clock();
    poll_once(relay.as_ref(), &book, &now).await.unwrap();

    decide(
        relay.as_ref(),
        &book,
        "spammer",
        ContactDecision::Block,
        false,
        &now,
    )
    .await
    .unwrap();

    let request = book.get("spammer").unwrap();
    assert_eq!(request.state, RequestState::Blocked);
    assert!(
        !request.state.is_actionable(),
        "a block is not casually undone"
    );
}

#[test]
fn a_blank_agent_id_is_never_recorded() {
    let book = ContactBook::default();
    assert!(!book.observe("", None, 1));
    assert!(!book.observe("   ", None, 1));
    assert!(book.requests().is_empty());
}

// ------------------------------------------------------------ poll health ---

#[tokio::test]
async fn a_failing_relay_is_reported_rather_than_looking_like_an_empty_queue() {
    // The bug this prevents: the poll loop swallowed every error, so a relay
    // that could not be reached showed exactly what "nobody has asked" shows —
    // an empty list — and the operator had no way to tell which.
    use super::desk::{ContactDesk, PollHealth};

    let relay = FakeRelay::with_incoming(&[]);
    *relay.fail.lock().unwrap() = true;
    let desk = ContactDesk::new(
        relay.clone() as std::sync::Arc<dyn ContactRelay>,
        AdmissionPolicy::Manual,
        Vec::<String>::new(),
    )
    .with_now(clock());

    assert_eq!(desk.health(), PollHealth::Pending, "nothing attempted yet");

    let health = desk.refresh().await;
    assert!(health.is_failing(), "got {health:?}");
    assert!(
        desk.health().summary(200).contains("unreachable"),
        "the summary must say so: {}",
        desk.health().summary(200)
    );
}

#[tokio::test]
async fn a_successful_poll_records_when_and_how_many() {
    use super::desk::{ContactDesk, PollHealth};

    let relay = FakeRelay::with_incoming(&["alice", "bob"]);
    let desk = ContactDesk::new(
        relay as std::sync::Arc<dyn ContactRelay>,
        AdmissionPolicy::Manual,
        Vec::<String>::new(),
    )
    .with_now(clock());

    let health = desk.refresh().await;
    match health {
        PollHealth::Ok { seen, .. } => assert_eq!(seen, 2),
        other => panic!("expected Ok, got {other:?}"),
    }
    assert!(!desk.health().is_failing());
    assert_eq!(desk.pending_count(), 2);
}

#[tokio::test]
async fn a_log_attached_to_one_handle_narrates_from_every_handle() {
    // The desk documents itself as cheap to clone with the clones sharing one
    // book — but the sink was per-instance, so the handle that polls and the
    // handle you attach a log to had to be the same value. They are not: the
    // service spawns the poll and hands a clone to the screen, which is what
    // wants the narration. Working around it by polling the clone gave two
    // polls of one relay: doubled traffic, interleaved snapshots, and duplicate
    // "new request(s)" lines.
    let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let relay = FakeRelay::with_incoming(&["alice"]);
    let polling = ContactDesk::new(
        relay.clone() as Arc<dyn ContactRelay>,
        AdmissionPolicy::Manual,
        Vec::<String>::new(),
    )
    .with_now(clock());

    // A different handle entirely — as `TinyplaceService::contacts()` hands out.
    let _screen = polling.clone().with_log({
        let seen = seen.clone();
        Arc::new(move |line: &str| seen.lock().unwrap().push(line.to_string()))
    });

    polling.refresh().await;

    let lines = seen.lock().unwrap().clone();
    assert!(
        lines.iter().any(|l| l.contains("1 new request")),
        "the poll must narrate through a sink attached to another handle: {lines:?}"
    );
}

#[tokio::test]
async fn contact_activity_is_narrated_so_silence_is_readable() {
    // A worker that receives nothing and a worker nobody has asked for look
    // identical in the UI. Only one of them is a problem with the worker.
    use super::desk::ContactDesk;
    use std::sync::Mutex as StdMutex;

    let seen: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
    let relay = FakeRelay::with_incoming(&["alice"]);
    let desk = ContactDesk::new(
        relay.clone() as Arc<dyn ContactRelay>,
        AdmissionPolicy::Manual,
        Vec::<String>::new(),
    )
    .with_now(clock())
    .with_log({
        let seen = seen.clone();
        Arc::new(move |line: &str| seen.lock().unwrap().push(line.to_string()))
    });

    desk.refresh().await;
    let lines = seen.lock().unwrap().clone();
    assert!(
        lines.iter().any(|l| l.contains("1 new request")),
        "an arrival must be recorded: {lines:?}"
    );

    // A second poll finding the same queue says nothing: a line every 1.5s
    // would drown the log it exists to explain.
    seen.lock().unwrap().clear();
    desk.refresh().await;
    assert!(seen.lock().unwrap().is_empty(), "no news, no line");
}

#[tokio::test]
async fn a_dead_relay_says_so_once_rather_than_every_tick() {
    use super::desk::ContactDesk;
    use std::sync::Mutex as StdMutex;

    let seen: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
    let relay = FakeRelay::with_incoming(&[]);
    *relay.fail.lock().unwrap() = true;
    let desk = ContactDesk::new(
        relay.clone() as Arc<dyn ContactRelay>,
        AdmissionPolicy::Manual,
        Vec::<String>::new(),
    )
    .with_now(clock())
    .with_log({
        let seen = seen.clone();
        Arc::new(move |line: &str| seen.lock().unwrap().push(line.to_string()))
    });

    desk.refresh().await;
    desk.refresh().await;
    desk.refresh().await;

    let lines = seen.lock().unwrap().clone();
    assert_eq!(lines.len(), 1, "said once, not per tick: {lines:?}");
    assert!(lines[0].contains("unreachable"), "got {:?}", lines[0]);
}

// --------------------------------------------------------------- contacts ---

#[tokio::test]
async fn established_contacts_are_fetched_not_inferred_from_the_request_queue() {
    // Regression. Accepted contacts were derived by filtering the queue of
    // *incoming requests*, but a contact the relay has accepted is no longer a
    // pending request — so it never appeared in that queue. The Contacts tab
    // was therefore empty on every fresh start no matter how many peers this
    // agent had, and never listed a peer this agent had requested and who
    // accepted, because that request never arrived here at all.
    let relay = FakeRelay::with_contacts(&["peer-established"]);
    let desk = ContactDesk::new(relay.clone(), AdmissionPolicy::Manual, Vec::<String>::new())
        .with_now(Arc::new(|| 1_000));

    assert!(desk.accepted().is_empty(), "nothing known before a poll");
    desk.refresh().await;

    let contacts = desk.accepted();
    assert_eq!(contacts.len(), 1, "the relay's contact must be listed");
    assert_eq!(contacts[0].agent_id, "peer-established");
    assert_eq!(contacts[0].state, RequestState::Accepted);
    assert_eq!(
        desk.pending_count(),
        0,
        "an established contact is not something waiting on the operator"
    );
    assert!(
        relay.calls().is_empty(),
        "a relationship that already exists must not be decided again: {:?}",
        relay.calls()
    );
}

#[tokio::test]
async fn an_auto_admit_policy_does_not_re_decide_an_existing_contact() {
    // `observe` inserts as pending, which under an open policy would spend an
    // accept call on a peer that is already a contact. `record_contact` exists
    // to avoid exactly that.
    let relay = FakeRelay::with_contacts(&["peer-established"]);
    let desk = ContactDesk::new(relay.clone(), AdmissionPolicy::All, Vec::<String>::new())
        .with_now(Arc::new(|| 1_000));

    desk.refresh().await;
    desk.refresh().await;

    assert_eq!(desk.accepted().len(), 1, "idempotent across polls");
    assert!(relay.calls().is_empty(), "got {:?}", relay.calls());
}

#[tokio::test]
async fn a_contact_is_announced_once_rather_than_every_poll() {
    let relay = FakeRelay::with_contacts(&["peer-established"]);
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let desk = ContactDesk::new(relay, AdmissionPolicy::Manual, Vec::<String>::new())
        .with_now(Arc::new(|| 1_000))
        .with_log({
            let seen = seen.clone();
            Arc::new(move |line: &str| seen.lock().unwrap().push(line.to_string()))
        });

    desk.refresh().await;
    desk.refresh().await;

    let lines = seen.lock().unwrap().clone();
    assert_eq!(lines.len(), 1, "said once, not per tick: {lines:?}");
    assert!(lines[0].contains("contact(s)"), "got {:?}", lines[0]);
    assert!(
        !lines[0].contains("request"),
        "a contact is not a pending request: {:?}",
        lines[0]
    );
}

#[tokio::test]
async fn accepting_re_reads_the_contact_list_rather_than_waiting_for_the_poll() {
    // The local settle already marks the peer accepted, so the row appears
    // either way — what this pins is that the relay was asked. The relay is
    // authoritative about the contact graph, and a decision is exactly the
    // moment that graph changed.
    let relay = FakeRelay::with_incoming(&["alice"]);
    let desk = ContactDesk::new(relay.clone(), AdmissionPolicy::Manual, Vec::<String>::new())
        .with_now(Arc::new(|| 1_000));
    desk.refresh().await;
    assert!(desk.accepted().is_empty(), "not a contact yet");

    let before = relay.trace().len();
    desk.decide("alice", ContactDecision::Accept).await.unwrap();

    // The assertion that matters: a list read *after* the accept. Without it
    // this test would pass on the local settle alone and prove nothing.
    let after: Vec<String> = relay.trace().into_iter().skip(before).collect();
    let accepted_at = after.iter().position(|c| c == "accept:alice");
    let listed_at = after.iter().rposition(|c| c == "list");
    assert!(
        matches!((accepted_at, listed_at), (Some(a), Some(l)) if l > a),
        "the contact list must be re-read after the accept, got {after:?}"
    );

    let contacts = desk.accepted();
    assert_eq!(contacts.len(), 1);
    assert_eq!(contacts[0].agent_id, "alice");
    assert_eq!(
        desk.pending_count(),
        0,
        "and it is no longer waiting on the operator"
    );
}

#[tokio::test]
async fn a_failed_re_read_does_not_turn_a_successful_accept_into_an_error() {
    // The decision already landed on the relay. Reporting it as failed would
    // leave the operator believing a peer was not admitted when it was — and
    // the background poll will reconcile the list a moment later anyway.
    let relay = FakeRelay::with_incoming(&["alice"]);
    let desk = ContactDesk::new(relay.clone(), AdmissionPolicy::Manual, Vec::<String>::new())
        .with_now(Arc::new(|| 1_000));
    desk.refresh().await;

    *relay.fail_list.lock().unwrap() = true;
    let line = desk.decide("alice", ContactDecision::Accept).await;

    assert!(
        line.is_ok(),
        "the accept succeeded, so this must too: {line:?}"
    );
    assert!(
        relay.calls().contains(&"accept:alice".to_string()),
        "got {:?}",
        relay.calls()
    );
    assert!(
        desk.accepted().iter().any(|c| c.agent_id == "alice"),
        "the local settle still stands"
    );
}
