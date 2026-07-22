//! Policy evaluation and the idempotent pending queue.

use super::*;

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
