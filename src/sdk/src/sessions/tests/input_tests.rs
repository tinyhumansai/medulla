//! Driver-seam tests: task frames and session envelopes folded into one
//! normalized turn, and the asymmetry between the two drivers.

use ::tinyplace::types::AnySessionEnvelope;

use crate::tinyplace::{HarnessProvider, TaskFrame, TaskFrameKind, TINYPLACE_PROTO};

use super::super::input::{envelope_turn, fold, Folded, SessionInput};
use super::super::types::{SessionClass, SessionDriver, SessionPolicy, TurnOrigin};

// ------------------------------------------------------------ driver seam ---

fn task_frame(kind: TaskFrameKind, task_id: &str, text: &str) -> TaskFrame {
    TaskFrame {
        proto: TINYPLACE_PROTO.to_string(),
        kind,
        task_id: task_id.to_string(),
        text: text.to_string(),
        ts: "2026-07-21T00:00:00Z".to_string(),
        correlation_id: Some("cyc/t1/abcd".to_string()),
        harness: None,
        provider: None,
        model: None,
        usage: None,
    }
}

#[test]
fn a_task_frame_folds_to_a_bounded_turn_anchored_on_the_authenticated_sender() {
    let folded = fold(
        SessionInput::Frame {
            from: "peer-crypto-id".to_string(),
            frame: task_frame(TaskFrameKind::Task, "t1", "ship it"),
        },
        HarnessProvider::Claude,
        SessionPolicy::Auto,
    );
    let Folded::Turn(turn) = folded else {
        panic!("a task frame must open a turn, got {folded:?}");
    };
    assert_eq!(turn.class, SessionClass::Bounded);
    assert_eq!(turn.key.conversation, "peer-crypto-id");
    assert_eq!(turn.text, "ship it");
    assert_eq!(
        turn.origin,
        TurnOrigin::Frame {
            task_id: "t1".to_string(),
            correlation_id: Some("cyc/t1/abcd".to_string()),
        }
    );
    assert_eq!(turn.origin.driver(), SessionDriver::Task);
}

#[test]
fn an_input_frame_folds_to_steering_not_a_new_turn() {
    let folded = fold(
        SessionInput::Frame {
            from: "peer".to_string(),
            frame: task_frame(TaskFrameKind::Input, "t1", "actually, use rust"),
        },
        HarnessProvider::Claude,
        SessionPolicy::Auto,
    );
    assert!(
        matches!(folded, Folded::Steer { ref task_id, .. } if task_id == "t1"),
        "got {folded:?}"
    );
}

#[test]
fn response_frames_fold_to_nothing() {
    for kind in [
        TaskFrameKind::Reply,
        TaskFrameKind::Status,
        TaskFrameKind::Error,
        TaskFrameKind::Ack,
    ] {
        let folded = fold(
            SessionInput::Frame {
                from: "peer".to_string(),
                frame: task_frame(kind, "t1", "…"),
            },
            HarnessProvider::Claude,
            SessionPolicy::Auto,
        );
        assert!(matches!(folded, Folded::Ignore), "{kind:?} → {folded:?}");
    }
}

#[test]
fn a_plain_text_dm_folds_to_an_unbound_turn() {
    let folded = fold(
        SessionInput::PlainText {
            from: "peer".to_string(),
            text: "hey, remember what we discussed?".to_string(),
        },
        HarnessProvider::Claude,
        SessionPolicy::Auto,
    );
    let Folded::Turn(turn) = folded else {
        panic!("expected a turn, got {folded:?}");
    };
    assert_eq!(turn.class, SessionClass::Unbound);
}

/// A v2 envelope carrying one `user_prompt`.
pub(super) fn prompt_envelope(wrapper: &str, harness: &str, text: &str) -> AnySessionEnvelope {
    let body = serde_json::json!({
        "envelope_version": "tinyplace.harness.session.v2",
        "version": 2,
        "bucket": { "unit": "hour", "start": "s", "end": "e" },
        "scope": {
            "type": "session", "key": "k", "cwd": "/repo",
            "wrapper_session_id": wrapper, "harness_session_id": harness,
        },
        "harness": { "provider": "claude", "command": "claude", "argv": [] },
        "event": {
            "id": "evt-1", "seq": 7, "ts": "2026-07-21T00:00:00Z",
            "role": "owner", "kind": "user_prompt",
            "payload": { "text": text, "source": "human" },
        },
        "source": { "path": "p", "record_type": "jsonl:user" },
    })
    .to_string();
    AnySessionEnvelope::parse(&body).expect("the sample envelope must parse")
}

#[test]
fn an_envelope_is_observed_never_executed() {
    // Folding an envelope must never produce a Turn: a remote wrapper is
    // already running that harness, and running it again here would duplicate
    // the work.
    let envelope = prompt_envelope("wrap-1", "harn-1", "write the docs");
    let folded = fold(
        SessionInput::Envelope(Box::new(envelope)),
        HarnessProvider::Claude,
        SessionPolicy::Auto,
    );
    let Folded::Observe(observation) = folded else {
        panic!("an envelope must fold to an observation, got {folded:?}");
    };
    assert_eq!(observation.key.conversation, "wrap-1");
    assert_eq!(observation.harness_session_id, "harn-1");
    assert_eq!(observation.seq, 7);
}

#[test]
fn the_envelope_anchor_falls_back_to_the_harness_session_id() {
    let envelope = prompt_envelope("", "harn-only", "hi");
    let Folded::Observe(observation) = crate::sessions::input::fold_envelope(&envelope) else {
        panic!("expected an observation");
    };
    assert_eq!(observation.key.conversation, "harn-only");
}

#[test]
fn envelope_turn_lifts_a_prompt_into_an_executable_turn() {
    // The opt-in escape hatch: a caller that *wants* to mirror the prompt
    // locally asks for it explicitly.
    let envelope = prompt_envelope("wrap-1", "harn-1", "write the docs");
    let turn = envelope_turn(&envelope, SessionClass::Unbound).expect("a prompt lifts");
    assert_eq!(turn.text, "write the docs");
    assert_eq!(turn.origin.driver(), SessionDriver::Envelope);
    assert_eq!(
        turn.origin,
        TurnOrigin::Envelope {
            event_id: "evt-1".to_string(),
            seq: 7,
        }
    );
}

#[test]
fn envelope_turn_refuses_a_non_prompt_event() {
    let body = serde_json::json!({
        "envelope_version": "tinyplace.harness.session.v2",
        "version": 2,
        "bucket": { "unit": "hour", "start": "s", "end": "e" },
        "scope": {
            "type": "session", "key": "k", "cwd": "/repo",
            "wrapper_session_id": "w", "harness_session_id": "h",
        },
        "harness": { "provider": "claude", "command": "claude", "argv": [] },
        "event": {
            "id": "e", "seq": 1, "ts": "t", "role": "agent",
            "kind": "agent_message", "payload": { "text": "done" },
        },
        "source": { "path": "p", "record_type": "jsonl:assistant" },
    })
    .to_string();
    let envelope = AnySessionEnvelope::parse(&body).expect("parses");
    assert!(envelope_turn(&envelope, SessionClass::Unbound).is_none());
}
