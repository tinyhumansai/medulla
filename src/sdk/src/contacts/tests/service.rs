//! Decision execution: what reaches the relay, and what it records.

use super::*;

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

// ------------------------------------------------------- contact reconcile ---

use super::super::service::{reconcile_contacts, spawn_contact_poll};
use std::time::Duration;

#[tokio::test]
async fn reconcile_contacts_records_established_peers_and_counts_only_new_ones() {
    let relay = FakeRelay::with_contacts(&["a", "b"]);
    let book = ContactBook::default();
    let now = clock();

    assert_eq!(
        reconcile_contacts(relay.as_ref(), &book, &now)
            .await
            .unwrap(),
        2,
        "both established contacts are new to a fresh book"
    );
    assert!(book.is_accepted("a") && book.is_accepted("b"));

    // The relay keeps listing them; a second pass must add nothing.
    assert_eq!(
        reconcile_contacts(relay.as_ref(), &book, &now)
            .await
            .unwrap(),
        0,
        "re-reading the same list is not news"
    );
}

#[tokio::test]
async fn reconcile_contacts_is_additive_a_contact_the_relay_drops_is_kept() {
    // `list` is paginated, so a truncated page must never strip a real contact:
    // losing a contact is worse than carrying a stale one.
    let relay = FakeRelay::with_contacts(&["a", "b"]);
    let book = ContactBook::default();
    let now = clock();
    reconcile_contacts(relay.as_ref(), &book, &now)
        .await
        .unwrap();

    // The next page omits "b".
    relay
        .accepted
        .lock()
        .unwrap()
        .retain(|contact| contact.agent_id == "a");
    reconcile_contacts(relay.as_ref(), &book, &now)
        .await
        .unwrap();

    assert!(
        book.is_accepted("b"),
        "a contact the relay stopped listing must not be demoted"
    );
}

#[tokio::test]
async fn a_poll_against_an_unreachable_relay_surfaces_the_error() {
    // A down relay is an error, not an empty queue — the poll must not swallow it
    // and leave the operator staring at a queue that merely looks quiet.
    let relay = FakeRelay::with_incoming(&["alice"]);
    *relay.fail.lock().unwrap() = true;
    let book = ContactBook::default();
    let now = clock();

    let outcome = poll_once(relay.as_ref(), &book, &now).await;
    assert!(outcome.is_err(), "the failure propagates out of poll_once");
    assert!(
        book.requests().is_empty(),
        "nothing was observed against a relay that could not be reached"
    );
}

#[tokio::test]
async fn spawn_contact_poll_fills_the_shared_book_and_stops_when_aborted() {
    let relay = FakeRelay::with_incoming(&["alice"]);
    let book = ContactBook::default();
    let handle = spawn_contact_poll(
        relay as Arc<dyn ContactRelay>,
        book.clone(),
        Duration::from_millis(1),
        clock(),
    );

    // The loop runs on its own; wait until it has observed the request.
    for _ in 0..200 {
        if book.pending_count() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert_eq!(
        book.pending_count(),
        1,
        "the spawned loop fills the book the caller kept a clone of"
    );

    handle.abort();
    assert!(
        handle.await.unwrap_err().is_cancelled(),
        "aborting the handle ends the loop"
    );
}
