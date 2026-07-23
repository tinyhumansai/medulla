//! What a poll reports about itself — and where it narrates.

use super::*;

// ------------------------------------------------------------ poll health ---

#[tokio::test]
async fn a_failing_relay_is_reported_rather_than_looking_like_an_empty_queue() {
    // The bug this prevents: the poll loop swallowed every error, so a relay
    // that could not be reached showed exactly what "nobody has asked" shows —
    // an empty list — and the operator had no way to tell which.
    use super::super::desk::{ContactDesk, PollHealth};

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

#[test]
fn a_never_polled_desk_says_so_rather_than_looking_healthy() {
    // Before the first poll the header must not read like a successful check of
    // an empty queue — the two are not the same and the operator can act on the
    // difference.
    use super::super::desk::PollHealth;
    assert_eq!(PollHealth::Pending.summary(1_000), "not polled yet");
}

#[tokio::test]
async fn declining_and_blocking_report_their_own_outcome_lines() {
    // Each decision returns the status line the UI shows; a decline and a block
    // must each name what happened rather than borrowing the accept wording.
    use super::super::desk::ContactDesk;

    let relay = FakeRelay::with_incoming(&["alice", "spammer"]);
    let desk = ContactDesk::new(
        relay.clone() as Arc<dyn ContactRelay>,
        AdmissionPolicy::Manual,
        Vec::<String>::new(),
    )
    .with_now(clock());
    desk.refresh().await;

    let declined = desk
        .decide("alice", ContactDecision::Decline)
        .await
        .expect("a decline succeeds against the relay");
    assert!(
        declined.contains("alice") && declined.contains("declined"),
        "{declined}"
    );

    let blocked = desk
        .decide("spammer", ContactDecision::Block)
        .await
        .expect("a block succeeds against the relay");
    assert!(
        blocked.contains("spammer") && blocked.contains("blocked"),
        "{blocked}"
    );

    assert!(
        relay.calls().contains(&"decline:alice".to_string())
            && relay.calls().contains(&"block:spammer".to_string()),
        "each decision must reach the relay: {:?}",
        relay.calls()
    );
}

#[tokio::test]
async fn a_successful_poll_records_when_and_how_many() {
    use super::super::desk::{ContactDesk, PollHealth};

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
    use super::super::desk::ContactDesk;
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
    use super::super::desk::ContactDesk;
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
