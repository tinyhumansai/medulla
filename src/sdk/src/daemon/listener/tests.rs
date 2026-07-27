//! Unit tests for the push inbox: what the socket's frames decode to, what the
//! delivery queue guarantees, and the deduplication that makes redelivery safe.
//!
//! The socket itself is not exercised here — that needs a relay, and the live
//! suites are where one belongs. What is testable offline is everything that
//! decides whether an envelope is processed, processed twice, or missed.

use std::collections::HashMap;
use std::time::Duration;

use super::{envelopes_in_frame, ws_inbox_enabled, PushInbox, SeenIds};

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// A relay envelope as it appears inside a stream frame.
fn envelope(id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "from": "peerA",
        "to": "me",
        "timestamp": "2026-01-01T00:00:00Z",
        "deviceId": 1,
        "type": "CIPHERTEXT",
        "body": "Y2lwaGVy",
    })
}

#[test]
fn the_push_channel_is_on_unless_explicitly_disabled() {
    assert!(ws_inbox_enabled(&env(&[])));
    assert!(ws_inbox_enabled(&env(&[("MEDULLA_INBOX_WS", "1")])));
    for off in ["0", "false", "off", " off "] {
        assert!(
            !ws_inbox_enabled(&env(&[("MEDULLA_INBOX_WS", off)])),
            "{off} should disable it"
        );
    }
}

// --- decoding what the socket carries --------------------------------------

#[test]
fn a_snapshot_frame_yields_the_whole_mailbox() {
    // The relay opens every stream with the un-acked backlog.
    let frame = serde_json::json!({
        "type": "snapshot",
        "data": { "to": "me", "messages": [envelope("m1"), envelope("m2")] },
        "sentAt": "2026-01-01T00:00:00Z",
    });
    let envelopes = envelopes_in_frame(&frame).expect("a snapshot decodes");
    assert_eq!(envelopes.len(), 2);
    assert_eq!(envelopes[0].id, "m1");
    assert_eq!(envelopes[1].id, "m2");
}

#[test]
fn a_live_frame_yields_the_one_relayed_envelope() {
    let frame = serde_json::json!({
        "type": "a2a.message",
        "data": envelope("m3"),
        "sentAt": "2026-01-01T00:00:00Z",
    });
    let envelopes = envelopes_in_frame(&frame).expect("a live event decodes");
    assert_eq!(envelopes.len(), 1);
    assert_eq!(envelopes[0].id, "m3");
    assert_eq!(envelopes[0].body, "Y2lwaGVy");
}

#[test]
fn an_empty_snapshot_is_an_empty_mailbox_not_a_failure() {
    // Distinct from an unreadable frame: this one positively says "nothing
    // here", so it must not force a fetch.
    let frame = serde_json::json!({
        "type": "snapshot",
        "data": { "to": "me", "messages": [] },
    });
    assert_eq!(envelopes_in_frame(&frame).map(|e| e.len()), Some(0));
}

#[test]
fn an_unreadable_frame_yields_nothing_so_the_caller_fetches() {
    // A keepalive, a frame kind added later, or a malformed payload. None may
    // be mistaken for an empty mailbox.
    for frame in [
        serde_json::json!({ "type": "some.future.event", "data": {} }),
        serde_json::json!({ "data": { "messages": [] } }),
        serde_json::json!({ "type": "snapshot" }),
        serde_json::json!({ "type": "a2a.message", "data": "not an envelope" }),
        serde_json::json!("not an object"),
    ] {
        assert!(
            envelopes_in_frame(&frame).is_none(),
            "should not decode: {frame}"
        );
    }
}

// --- the delivery queue ----------------------------------------------------

#[tokio::test]
async fn a_delivery_wakes_a_waiter_and_can_be_taken() {
    let inbox = PushInbox::new();
    let deliverer = inbox.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        deliverer.deliver(vec![serde_json::from_value(envelope("m1")).unwrap()]);
    });

    tokio::time::timeout(Duration::from_secs(5), inbox.wait(Duration::from_secs(60)))
        .await
        .expect("the delivery should end the wait, not the poll");
    let taken = inbox.take();
    assert_eq!(taken.len(), 1);
    assert!(inbox.take().is_empty(), "taking twice must not duplicate");
}

#[tokio::test]
async fn a_delivery_with_nobody_waiting_is_not_lost() {
    // An envelope arriving mid-drain must not wait out a whole poll interval.
    let inbox = PushInbox::new();
    inbox.deliver(vec![serde_json::from_value(envelope("m1")).unwrap()]);
    tokio::time::timeout(Duration::from_secs(5), inbox.wait(Duration::from_secs(60)))
        .await
        .expect("the earlier delivery should still be pending");
    assert_eq!(inbox.take().len(), 1);
}

#[tokio::test]
async fn the_poll_still_ends_the_wait_when_nothing_arrives() {
    // The timeout is the correctness floor, not a fallback.
    let inbox = PushInbox::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        inbox.wait(Duration::from_millis(20)),
    )
    .await
    .expect("the poll must still fire on its own");
}

#[test]
fn an_overflowing_queue_forces_a_fetch_rather_than_growing() {
    // A stalled drain must not let this grow without bound. Nothing is lost:
    // the envelopes are still in the mailbox until they are acknowledged.
    let inbox = PushInbox::new();
    let flood: Vec<_> = (0..1_000)
        .map(|n| serde_json::from_value(envelope(&format!("m{n}"))).unwrap())
        .collect();
    inbox.deliver(flood);

    assert!(inbox.take().len() <= super::MAX_QUEUED);
    assert!(
        inbox.take_must_fetch(),
        "overflow must leave a fetch owed so the remainder is picked up"
    );
}

#[test]
fn a_nudge_owes_a_fetch_without_delivering_anything() {
    let inbox = PushInbox::new();
    inbox.nudge();
    assert!(inbox.take().is_empty());
    assert!(inbox.take_must_fetch());
    assert!(!inbox.take_must_fetch(), "the flag is cleared by taking it");
}

#[test]
fn a_fresh_inbox_is_not_listening_and_owes_nothing() {
    let inbox = PushInbox::new();
    assert!(!inbox.is_listening());
    assert!(!inbox.take_must_fetch());
}

// --- deduplication ---------------------------------------------------------

#[test]
fn the_same_message_is_only_accepted_once() {
    // The case this exists for: a reconnect replays the snapshot, so an
    // envelope legitimately arrives twice. Handing a screen delta to the fold
    // twice would fail its base_seq check and force a needless resync.
    let seen = SeenIds::new();
    assert!(seen.insert("m1"));
    assert!(!seen.insert("m1"));
    assert!(seen.insert("m2"));
}

#[test]
fn an_empty_id_is_never_treated_as_a_duplicate() {
    // The relay rejects empty ids, so one appearing here is malformed rather
    // than a repeat — collapsing them all onto each other would drop real
    // traffic.
    let seen = SeenIds::new();
    assert!(seen.insert(""));
    assert!(seen.insert(""));
}

#[test]
fn the_seen_set_is_bounded_and_forgets_the_oldest() {
    // A long-lived daemon must not accumulate every id it ever saw. Forgetting
    // is safe: an envelope that old was acknowledged long ago and will not be
    // redelivered.
    let seen = SeenIds::new();
    for n in 0..(super::SEEN_CAPACITY + 10) {
        assert!(seen.insert(&format!("m{n}")));
    }
    assert!(
        seen.insert("m0"),
        "the oldest id should have been forgotten"
    );
    assert!(
        !seen.insert(&format!("m{}", super::SEEN_CAPACITY + 5)),
        "a recent id is still remembered"
    );
}
