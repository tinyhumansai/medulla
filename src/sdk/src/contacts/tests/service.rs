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

#[tokio::test]
async fn a_master_paired_mid_session_is_admitted_without_a_restart() {
    // The bug this prevents: the allowlist was seeded once, from the peers the
    // config named at boot. Pairing a master from the daemon's Master tab wrote
    // the config and told the operator it had worked, but the running desk had
    // never heard of it — so the master's answering contact request queued as a
    // stranger's, the Master row never came up, and the pairing only completed
    // the next time the worker was restarted.
    let relay = FakeRelay::with_incoming(&["master"]);
    let desk = ContactDesk::new(
        relay.clone() as Arc<dyn ContactRelay>,
        AdmissionPolicy::Allowlist,
        Vec::<String>::new(),
    )
    .with_now(clock());

    desk.refresh().await;
    assert_eq!(desk.pending_count(), 1, "not paired yet, so nothing admits");

    desk.allow("master");
    desk.refresh().await;

    assert_eq!(relay.calls(), vec!["accept:master".to_string()]);
    assert_eq!(desk.pending_count(), 0);
    assert!(
        desk.accepted().iter().any(|c| c.agent_id == "master"),
        "the paired master is a contact now: {:?}",
        desk.accepted()
    );
}

#[tokio::test]
async fn an_established_contact_is_narrated_by_id_not_only_counted() {
    // "1 new contact" is not answerable: a peer that pairs itself settles with
    // nobody deciding anything here, so the log has to say which peer it was.
    let relay = FakeRelay::with_incoming(&["master"]);
    let said = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = {
        let said = said.clone();
        Arc::new(move |line: &str| said.lock().unwrap().push(line.to_string()))
    };
    let desk = ContactDesk::new(
        relay.clone() as Arc<dyn ContactRelay>,
        AdmissionPolicy::All,
        Vec::<String>::new(),
    )
    .with_now(clock())
    .with_log(sink);

    desk.refresh().await;

    let lines = said.lock().unwrap().clone();
    assert!(
        lines.iter().any(|line| line.contains("master")),
        "got {lines:?}"
    );

    // A second poll finds the same contact and must not narrate it again.
    said.lock().unwrap().clear();
    desk.refresh().await;
    assert!(said.lock().unwrap().is_empty(), "got {:?}", said.lock());
}

#[tokio::test]
async fn an_automatic_accept_the_relay_refused_is_retried_on_the_next_poll() {
    // Where nobody is watching — the hub identity has no operator screen — a
    // single transient `accept` failure used to strand the peer for the life of
    // the process: `decide` recorded it as Failed, and only Pending records were
    // candidates for auto-admission.
    let relay = FakeRelay::with_incoming(&["worker"]);
    *relay.fail_decision.lock().unwrap() = true;
    let book = ContactBook::new(AdmissionPolicy::All, Vec::<String>::new());
    let now = clock();

    poll_once(relay.as_ref(), &book, &now).await.unwrap();
    assert_eq!(book.get("worker").unwrap().state, RequestState::Failed);

    *relay.fail_decision.lock().unwrap() = false;
    poll_once(relay.as_ref(), &book, &now).await.unwrap();

    assert_eq!(book.get("worker").unwrap().state, RequestState::Accepted);
}

#[tokio::test]
async fn an_operator_decision_the_relay_refused_is_not_retried_as_an_accept() {
    // The retry above must not reach across and re-decide for the operator: a
    // decline that failed is theirs to repeat, and silently converting it into
    // an auto-accept would be the opposite of what they asked for.
    let relay = FakeRelay::with_incoming(&["stranger"]);
    let book = ContactBook::default();
    let now = clock();
    poll_once(relay.as_ref(), &book, &now).await.unwrap();

    *relay.fail_decision.lock().unwrap() = true;
    let refused = decide(
        relay.as_ref(),
        &book,
        "stranger",
        ContactDecision::Decline,
        false,
        &now,
    )
    .await;
    assert!(refused.is_err());
    *relay.fail_decision.lock().unwrap() = false;

    book.set_policy(AdmissionPolicy::All);
    poll_once(relay.as_ref(), &book, &now).await.unwrap();

    assert_eq!(
        book.get("stranger").unwrap().state,
        RequestState::Failed,
        "an operator's failed decision waits for the operator"
    );
}
