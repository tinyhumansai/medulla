//! Driver-seam tests: task frames and session envelopes folded into one
//! normalized turn, and the asymmetry between the two drivers.

use ::tinyplace::types::{AnySessionEnvelope, SessionEnvelopeV1, SessionEnvelopeV2};

use crate::tinyplace::{HarnessProvider, TaskFrame, TaskFrameKind, TINYPLACE_PROTO};

use super::super::input::{envelope_turn, fold, Folded, Observation, SessionInput};
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

/// A v2 envelope carrying an arbitrary event `kind`/`payload`, anchored on a
/// fixed wrapper/harness id so folds are comparable across kinds.
fn v2_event_envelope(kind: &str, payload: serde_json::Value) -> AnySessionEnvelope {
    let body = serde_json::json!({
        "envelope_version": "tinyplace.harness.session.v2",
        "version": 2,
        "bucket": { "unit": "hour", "start": "s", "end": "e" },
        "scope": {
            "type": "session", "key": "k", "cwd": "/repo",
            "wrapper_session_id": "wrap-1", "harness_session_id": "harn-1",
        },
        "harness": { "provider": "claude", "command": "claude", "argv": [] },
        "event": {
            "id": "evt-1", "seq": 9, "ts": "2026-07-21T00:00:00Z",
            "role": "agent", "kind": kind, "payload": payload,
        },
        "source": { "path": "p", "record_type": "jsonl:assistant" },
    })
    .to_string();
    AnySessionEnvelope::parse(&body).expect("the sample envelope must parse")
}

/// A v1 envelope whose only semantic content is its `message.text`.
fn v1_envelope(wrapper: &str, harness: &str, text: &str) -> AnySessionEnvelope {
    let body = serde_json::json!({
        "envelope_version": "tinyplace.harness.session.v1",
        "version": 1,
        "bucket": { "unit": "hour", "start": "s", "end": "e" },
        "scope": {
            "type": "session", "key": "k", "cwd": "/repo",
            "wrapper_session_id": wrapper, "harness_session_id": harness,
        },
        "harness": { "provider": "claude", "command": "claude", "argv": [] },
        "message": {
            "id": "m1", "line": 4, "role": "agent", "text": text,
            "timestamp": "2026-07-02T00:00:00Z",
        },
        "source": { "path": "p", "record_type": "assistant" },
    })
    .to_string();
    AnySessionEnvelope::parse(&body).expect("valid v1 envelope")
}

fn observe(envelope: &AnySessionEnvelope) -> Observation {
    match crate::sessions::input::fold_envelope(envelope) {
        Folded::Observe(observation) => *observation,
        other => panic!("expected an observation, got {other:?}"),
    }
}

#[test]
fn a_blank_plain_text_dm_folds_to_nothing() {
    // A DM that is only whitespace must never open a turn — it would spawn a
    // harness with an empty prompt.
    let folded = fold(
        SessionInput::PlainText {
            from: "peer".to_string(),
            text: "   \n\t ".to_string(),
        },
        HarnessProvider::Claude,
        SessionPolicy::Auto,
    );
    assert!(matches!(folded, Folded::Ignore), "got {folded:?}");
}

#[test]
fn a_bounded_policy_pins_a_conversational_dm_to_bounded() {
    // Under `auto` a DM routes unbound; the operator's Bounded pin overrides
    // the stimulus so even a chatty message runs one-shot.
    let folded = fold(
        SessionInput::PlainText {
            from: "peer".to_string(),
            text: "just chatting".to_string(),
        },
        HarnessProvider::Claude,
        SessionPolicy::Bounded,
    );
    let Folded::Turn(turn) = folded else {
        panic!("expected a turn, got {folded:?}");
    };
    assert_eq!(turn.class, SessionClass::Bounded);
}

#[test]
fn a_frames_own_provider_overrides_the_default() {
    // The frame names codex; the default is claude. Bindings are per provider,
    // so the key must carry codex or a resume would cross harnesses.
    let mut frame = task_frame(TaskFrameKind::Task, "t1", "go");
    frame.provider = Some(HarnessProvider::Codex);
    let Folded::Turn(turn) = fold(
        SessionInput::Frame {
            from: "peer".to_string(),
            frame,
        },
        HarnessProvider::Claude,
        SessionPolicy::Auto,
    ) else {
        panic!("expected a turn");
    };
    assert_eq!(turn.key.provider, HarnessProvider::Codex);
}

#[test]
fn an_agent_message_envelope_observes_its_text_verbatim() {
    let obs = observe(&v2_event_envelope(
        "agent_message",
        serde_json::json!({ "text": "here is the answer" }),
    ));
    assert_eq!(obs.detail, "here is the answer");
    assert!(!obs.ends_turn, "an agent message does not end the turn");
    assert!(!obs.is_error);
    assert_eq!(obs.seq, 9);
}

#[test]
fn agent_thinking_is_observed_as_a_thinking_marker_not_its_content() {
    // The reasoning text is deliberately dropped; only that the agent is
    // thinking is surfaced.
    let obs = observe(&v2_event_envelope(
        "agent_thinking",
        serde_json::json!({ "text": "secret chain of thought" }),
    ));
    assert_eq!(obs.detail, "thinking");
}

#[test]
fn a_tool_call_envelope_names_the_tool() {
    let obs = observe(&v2_event_envelope(
        "tool_call",
        serde_json::json!({ "tool_name": "Bash", "tool_kind": "shell" }),
    ));
    assert_eq!(obs.detail, "tool Bash");
}

#[test]
fn a_failed_tool_result_is_marked_an_error_and_flagged() {
    // The ✕ suffix and the is_error flag must both flip only when the result
    // reports a failure.
    let ok = observe(&v2_event_envelope(
        "tool_result",
        serde_json::json!({ "is_error": false }),
    ));
    assert_eq!(ok.detail, "tool result");
    assert!(!ok.is_error);

    let failed = observe(&v2_event_envelope(
        "tool_result",
        serde_json::json!({ "is_error": true }),
    ));
    assert_eq!(failed.detail, "tool result ✕");
    assert!(failed.is_error);
}

#[test]
fn an_approval_request_envelope_names_the_tool() {
    let obs = observe(&v2_event_envelope(
        "approval_request",
        serde_json::json!({ "tool_name": "WriteFile" }),
    ));
    assert_eq!(obs.detail, "approval: WriteFile");
}

#[test]
fn a_status_envelope_carries_its_detail() {
    let obs = observe(&v2_event_envelope(
        "status",
        serde_json::json!({ "state": "running", "detail": "compiling" }),
    ));
    assert_eq!(obs.detail, "compiling");
}

#[test]
fn a_turn_end_lifecycle_advances_the_turn_counter_but_other_phases_do_not() {
    // Only `turn_end` ends a turn; a `turn_start` on the same code path must
    // not, or the turn counter would double-count.
    let ended = observe(&v2_event_envelope(
        "lifecycle",
        serde_json::json!({ "phase": "turn_end" }),
    ));
    assert_eq!(ended.detail, "lifecycle: turn_end");
    assert!(ended.ends_turn);

    let started = observe(&v2_event_envelope(
        "lifecycle",
        serde_json::json!({ "phase": "turn_start" }),
    ));
    assert!(!started.ends_turn);
}

#[test]
fn an_error_envelope_observes_a_failure() {
    let obs = observe(&v2_event_envelope(
        "error",
        serde_json::json!({ "message": "harness crashed", "fatal": true }),
    ));
    assert_eq!(obs.detail, "harness crashed");
    assert!(obs.is_error);
    assert!(!obs.ends_turn, "an error is not itself a turn boundary");
}

#[test]
fn an_unknown_v2_event_kind_folds_to_nothing() {
    // A forward-incompatible event decodes to Unknown and carries no state, so
    // it must be ignored rather than surfaced as a blank observation.
    let folded = crate::sessions::input::fold_envelope(&v2_event_envelope(
        "some_future_kind",
        serde_json::json!({ "whatever": 1 }),
    ));
    assert!(matches!(folded, Folded::Ignore), "got {folded:?}");
}

#[test]
fn a_v2_envelope_with_no_anchor_at_all_folds_to_nothing() {
    // Both wrapper and harness ids blank means there is no conversation to key
    // on; the envelope is unroutable and must be dropped. Built directly so the
    // defensive guard is reached even though the wire parser also rejects it.
    let envelope = AnySessionEnvelope::V2(SessionEnvelopeV2::default());
    assert!(matches!(
        crate::sessions::input::fold_envelope(&envelope),
        Folded::Ignore
    ));
}

#[test]
fn an_unknown_wire_provider_falls_back_to_claude() {
    // The wire provider is a free string for forward-compat; an unrecognized
    // one must not drop the envelope — it folds under the Claude fallback.
    let obs = observe(&v2_event_envelope_with_provider(
        "gemini-next",
        "agent_message",
        serde_json::json!({ "text": "hi" }),
    ));
    assert_eq!(obs.key.provider, HarnessProvider::Claude);
}

/// Like [`v2_event_envelope`] but with a caller-chosen wire provider string, to
/// exercise the fallback path.
fn v2_event_envelope_with_provider(
    provider: &str,
    kind: &str,
    payload: serde_json::Value,
) -> AnySessionEnvelope {
    let body = serde_json::json!({
        "envelope_version": "tinyplace.harness.session.v2",
        "version": 2,
        "bucket": { "unit": "hour", "start": "s", "end": "e" },
        "scope": {
            "type": "session", "key": "k", "cwd": "/repo",
            "wrapper_session_id": "wrap-1", "harness_session_id": "harn-1",
        },
        "harness": { "provider": provider, "command": "x", "argv": [] },
        "event": {
            "id": "evt-1", "seq": 9, "ts": "t",
            "role": "agent", "kind": kind, "payload": payload,
        },
        "source": { "path": "p", "record_type": "jsonl:assistant" },
    })
    .to_string();
    AnySessionEnvelope::parse(&body).expect("parses")
}

#[test]
fn a_v1_envelope_observes_its_message_text() {
    // v1 has no typed event; the message block is the whole payload and its
    // `line` stands in for the missing per-event sequence.
    let obs = observe(&v1_envelope("wrap-v1", "harn-v1", "legacy line"));
    assert_eq!(obs.key.conversation, "wrap-v1");
    assert_eq!(obs.detail, "legacy line");
    assert_eq!(obs.seq, 4, "v1 orders on the message line number");
    assert!(!obs.is_error);
}

#[test]
fn a_v1_envelope_with_blank_text_folds_to_nothing() {
    let folded = crate::sessions::input::fold_envelope(&v1_envelope("w", "h", "   "));
    assert!(matches!(folded, Folded::Ignore), "got {folded:?}");
}

#[test]
fn a_v1_envelope_with_no_anchor_folds_to_nothing() {
    // Built directly: a wire-parsed v1 always has a non-empty harness id, so
    // this exercises the same defensive anchor guard the parser would catch.
    let folded = crate::sessions::input::fold_envelope(&AnySessionEnvelope::V1(
        SessionEnvelopeV1::default(),
    ));
    assert!(matches!(folded, Folded::Ignore), "got {folded:?}");
}

#[test]
fn envelope_turn_refuses_a_v1_envelope() {
    // Only a v2 `user_prompt` carries an executable instruction; a v1 envelope
    // never lifts into a runnable turn.
    let envelope = v1_envelope("w", "h", "please run this");
    assert!(envelope_turn(&envelope, SessionClass::Unbound).is_none());
}

#[test]
fn envelope_turn_refuses_a_blank_prompt() {
    let envelope = prompt_envelope("wrap-1", "harn-1", "   ");
    assert!(envelope_turn(&envelope, SessionClass::Unbound).is_none());
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
