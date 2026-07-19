//! Unit tests for the core client's transport primitives: sequence-gap detection
//! ([`SeqTracker`]) and the frame decoders (`decode_event`, `decode_response`).

use serde_json::{json, Value};

use super::client::{decode_event, decode_response};
use super::types::SeqTracker;

#[test]
fn seq_tracker_detects_a_gap() {
    let mut t = SeqTracker::new(0);
    assert!(!t.observe(1));
    assert!(!t.observe(2));
    assert!(t.observe(10)); // core coalesced 3..9
    assert_eq!(t.last_seq(), 10);
    assert!(!t.observe(11));
}

#[test]
fn decode_event_reads_the_envelope() {
    let frame = json!({
        "t": "event", "seq": 5, "at": 42, "threadId": "th_x",
        "cycleId": "cyc:app:th_x:1", "event": {"kind": "assistant", "body": "hi"}
    });
    let ev = decode_event(&frame).unwrap();
    assert_eq!(ev.seq, 5);
    assert_eq!(ev.at, 42);
    assert_eq!(ev.kind(), "assistant");
    assert_eq!(ev.cycle_id, "cyc:app:th_x:1");
}

#[test]
fn decode_response_splits_ok_and_error() {
    let ok = json!({ "id": 1, "ok": { "threadId": "th_x" } });
    assert_eq!(
        decode_response(&ok)
            .unwrap()
            .get("threadId")
            .and_then(Value::as_str),
        Some("th_x")
    );
    let err = json!({ "id": 2, "error": { "code": "thread.not-found", "message": "no", "retryable": false } });
    let e = decode_response(&err).unwrap_err();
    assert_eq!(e.code, "thread.not-found");
    assert!(!e.retryable);
}
