//! Tests for the envelope module.

use super::*;
use crate::daemon::mappers::HarnessLineMapper;

fn builder() -> EnvelopeBuilder {
    EnvelopeBuilder::new(
        "wsid-1",
        "wsid-1",
        "/repo",
        "claude",
        "claude",
        vec!["-p".to_string()],
    )
}

#[test]
fn epoch_iso_round_trips() {
    // 2026-07-05T00:00:00.000Z is 1_783_209_600_000 ms.
    assert_eq!(
        epoch_ms_to_iso(1_783_209_600_000),
        "2026-07-05T00:00:00.000Z"
    );
    assert_eq!(
        epoch_ms_to_iso(1_783_209_600_500),
        "2026-07-05T00:00:00.500Z"
    );
    assert_eq!(epoch_ms_to_iso(0), "1970-01-01T00:00:00.000Z");
    // Round-trip through the mapper's parser.
    let ms = 1_783_209_671_123;
    assert_eq!(iso_to_epoch_ms(&epoch_ms_to_iso(ms)), Some(ms));
}

#[test]
fn bucket_floors_to_the_minute() {
    let bucket = minute_bucket("2026-07-05T12:34:56.789Z");
    assert_eq!(bucket.unit, "minute");
    assert_eq!(bucket.start, "2026-07-05T12:34:00.000Z");
    assert_eq!(bucket.end, "2026-07-05T12:35:00.000Z");
}

#[test]
fn event_id_is_stable_and_position_sensitive() {
    let a = event_id("wsid-1", "assistant:text", 3, "agent_message");
    let b = event_id("wsid-1", "assistant:text", 3, "agent_message");
    assert_eq!(a, b, "same inputs → same id");
    assert_ne!(a, event_id("wsid-1", "assistant:text", 4, "agent_message"));
    assert_ne!(a, event_id("wsid-2", "assistant:text", 3, "agent_message"));
    assert_eq!(a.len(), 64, "sha256 hex");
}

#[test]
fn seq_advances_and_stamps_ids() {
    let mut b = builder();
    assert_eq!(b.next_seq(), 0);
    let mut mapper = HarnessLineMapper::new_with_gh_repo_override("claude", false);
    let line = r#"{"type":"assistant","timestamp":"2026-07-05T00:00:00Z","message":{"role":"assistant","content":[{"type":"text","text":"hi"}]}}"#;
    let events = mapper.map_line(line, 0);
    let env0 = b.event_envelope(&events[0]);
    assert_eq!(env0.event.seq, 0);
    assert_eq!(env0.event.kind, "agent_message");
    assert_eq!(env0.scope.wrapper_session_id, "wsid-1");
    assert_eq!(env0.scope.scope_type, "session");
    assert_eq!(env0.harness.provider, "claude");
    assert_eq!(env0.source.record_type, "assistant:text");
    assert_eq!(
        env0.event.id,
        event_id("wsid-1", "assistant:text", 0, "agent_message")
    );
    // Next envelope advances seq.
    let env1 = b.event_envelope(&events[0]);
    assert_eq!(env1.event.seq, 1);
    assert_eq!(b.next_seq(), 2);
    // Wire round-trips as a valid v2 envelope.
    let wire = serde_json::to_string(&env0).unwrap();
    assert!(SessionEnvelopeV2::parse(&wire).is_some());
}

#[test]
fn harness_id_update_keeps_envelopes_valid() {
    let mut b = builder();
    b.set_harness_session_id("");
    assert_eq!(
        b.synthetic_envelope(
            HarnessEvent {
                kind: "lifecycle".to_string(),
                role: "agent".to_string(),
                payload: serde_json::json!({ "phase": "session_start" }),
                ..Default::default()
            },
            "wrapper:lifecycle",
            0,
        )
        .scope
        .harness_session_id,
        "wsid-1",
        "empty id ignored, fallback retained",
    );
    b.set_harness_session_id("real-id");
    let env = b.synthetic_envelope(
        HarnessEvent {
            kind: "lifecycle".to_string(),
            role: "agent".to_string(),
            payload: serde_json::json!({ "phase": "session_end" }),
            ..Default::default()
        },
        "wrapper:lifecycle",
        0,
    );
    assert_eq!(env.scope.harness_session_id, "real-id");
}
