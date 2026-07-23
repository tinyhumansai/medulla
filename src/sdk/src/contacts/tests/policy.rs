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
fn pending_lists_only_the_requests_still_awaiting_a_decision() {
    // The Requests tab renders this list, so a settled peer must drop out of it
    // even while it stays in the full first-seen history the queue keeps.
    let book = ContactBook::default();
    book.observe("waiting", None, 1);
    book.observe("decided", None, 1);
    book.settle("decided", ContactDecision::Accept, false, 2);

    let pending = book.pending();
    assert_eq!(pending.len(), 1, "only the undecided peer is pending");
    assert_eq!(pending[0].agent_id, "waiting");
    assert_eq!(pending[0].state, RequestState::Pending);
    assert_eq!(
        book.requests().len(),
        2,
        "the settled peer stays in the full history"
    );
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

#[test]
fn a_peer_allowed_after_construction_is_auto_admitted_under_allowlist() {
    let book = ContactBook::new(AdmissionPolicy::Allowlist, Vec::<String>::new());
    assert!(!book.is_allowed("late"));
    assert_eq!(
        book.auto_decision("late"),
        None,
        "unlisted is queued, not admitted"
    );

    book.allow("late");
    assert!(book.is_allowed("late"));
    assert_eq!(
        book.auto_decision("late"),
        Some(ContactDecision::Accept),
        "a peer added to the allowlist is now auto-admitted"
    );
}

#[test]
fn recording_an_existing_pending_request_as_a_contact_promotes_it_once() {
    // The relay is authoritative about the contact graph, so a peer it reports as
    // accepted overrides a still-pending local record — without re-announcing.
    let book = ContactBook::default();
    book.observe("peer", None, 1);
    assert_eq!(book.get("peer").unwrap().state, RequestState::Pending);

    assert!(
        book.record_contact("peer", None, 2),
        "a pending peer is promoted"
    );
    assert_eq!(book.get("peer").unwrap().state, RequestState::Accepted);
    assert_eq!(
        book.pending_count(),
        0,
        "a contact is not waiting on the operator"
    );

    assert!(
        !book.record_contact("peer", None, 3),
        "already a contact, so no news"
    );
    assert!(
        !book.record_contact("  ", None, 4),
        "a blank id is never recorded as a contact"
    );
}

#[test]
fn record_contact_inserts_a_fresh_contact_and_refreshes_a_late_handle() {
    let book = ContactBook::default();
    assert!(
        book.record_contact("peer", None, 1),
        "a contact unknown to the book is new"
    );
    assert_eq!(book.get("peer").unwrap().state, RequestState::Accepted);

    book.record_contact("peer", Some("@handle".to_string()), 2);
    assert_eq!(
        book.get("peer").unwrap().handle.as_deref(),
        Some("@handle"),
        "a handle learned later refreshes an established contact"
    );
}

#[test]
fn set_policy_changes_what_the_book_will_auto_admit() {
    let book = ContactBook::default();
    book.observe("peer", None, 1);
    assert_eq!(book.policy(), AdmissionPolicy::Manual);
    assert_eq!(book.auto_decision("peer"), None, "manual admits nothing");

    book.set_policy(AdmissionPolicy::All);
    assert_eq!(book.policy(), AdmissionPolicy::All);
    assert_eq!(
        book.auto_decision("peer"),
        Some(ContactDecision::Accept),
        "an open policy auto-admits every peer"
    );
}

// ------------------------------------------------------------------ types ---

#[test]
fn request_state_labels_and_glyphs_are_all_distinct() {
    use std::collections::HashSet;
    let states = [
        RequestState::Pending,
        RequestState::Accepting,
        RequestState::Accepted,
        RequestState::Declined,
        RequestState::Blocked,
        RequestState::Failed,
    ];
    let labels: HashSet<_> = states.iter().map(|state| state.as_str()).collect();
    let glyphs: HashSet<_> = states.iter().map(|state| state.glyph()).collect();
    assert_eq!(
        labels.len(),
        6,
        "each state needs a distinct label: {labels:?}"
    );
    assert_eq!(
        glyphs.len(),
        6,
        "each state needs a distinct glyph: {glyphs:?}"
    );

    // Only a pending or a (retryable) failed request is still actionable.
    assert!(RequestState::Pending.is_actionable());
    assert!(RequestState::Failed.is_actionable());
    assert!(!RequestState::Accepting.is_actionable());
    assert!(!RequestState::Accepted.is_actionable());
    assert!(!RequestState::Declined.is_actionable());
    assert!(!RequestState::Blocked.is_actionable());
}

#[test]
fn admission_policy_round_trips_through_its_wire_string() {
    for policy in [
        AdmissionPolicy::Manual,
        AdmissionPolicy::Allowlist,
        AdmissionPolicy::All,
    ] {
        assert_eq!(
            AdmissionPolicy::parse(policy.as_str()),
            policy,
            "{policy:?} must survive as_str -> parse"
        );
    }
    // Aliases and case are accepted; unknown values close.
    assert_eq!(AdmissionPolicy::parse("any"), AdmissionPolicy::All);
    assert_eq!(AdmissionPolicy::parse("PEERS"), AdmissionPolicy::Allowlist);
    assert_eq!(
        AdmissionPolicy::parse("configured"),
        AdmissionPolicy::Allowlist
    );
}

#[test]
fn each_decision_maps_to_its_settled_state_and_label() {
    assert_eq!(
        ContactDecision::Accept.settled_state(),
        RequestState::Accepted
    );
    assert_eq!(
        ContactDecision::Decline.settled_state(),
        RequestState::Declined
    );
    assert_eq!(
        ContactDecision::Block.settled_state(),
        RequestState::Blocked
    );

    assert_eq!(ContactDecision::Accept.as_str(), "accept");
    assert_eq!(ContactDecision::Decline.as_str(), "decline");
    assert_eq!(ContactDecision::Block.as_str(), "block");
}

#[test]
fn display_name_falls_back_to_the_id_when_no_handle_is_known() {
    let book = ContactBook::default();
    book.observe("crypto-id", None, 1);
    assert_eq!(
        book.get("crypto-id").unwrap().display_name(),
        "crypto-id",
        "with no handle the operator sees the raw id they are approving"
    );
}
