//! What the roster advert says about who holds a harness.
//!
//! The advert is the transport for the whole handoff feature: it is already
//! re-emitted on every roster mutation, so a control change is already an event.
//! These pin the two properties that makes safe — that the common case stays
//! byte-stable, and that a stale invitation is never advertised.

use super::super::roster::{register_payload, HubWorker};
use super::super::{HandoffControl, HarnessHandoff};

fn no_presence() -> std::collections::HashMap<String, bool> {
    std::collections::HashMap::new()
}

fn worker() -> HubWorker {
    HubWorker {
        id: "this-device".to_string(),
        address: "GRVaddr".to_string(),
        harness: "claude".to_string(),
        workspace: Some("/repos/acme".to_string()),
        ..Default::default()
    }
}

fn brief() -> HarnessHandoff {
    HarnessHandoff {
        id: "w_3-1".to_string(),
        at: 1_753_420_600_000,
        session_id: "w_3".to_string(),
        harness_session_id: None,
        provider: "claude".to_string(),
        workspace_path: "/repos/acme".to_string(),
        branch: Some("feat/login".to_string()),
        project: None,
        note: Some("stuck on the failing e2e".to_string()),
        transcript: "…pnpm test, 3 failures".to_string(),
        transcript_truncated: false,
    }
}

/// The advert for one worker's `metadata`.
fn metadata(w: HubWorker) -> serde_json::Value {
    register_payload(&[w], &no_presence(), &[])["agents"][0]["metadata"].clone()
}

#[test]
fn an_orchestrator_held_harness_says_nothing_about_control() {
    // Absent means orchestrator-held. Omitting the common case is what keeps
    // this advert byte-stable across the re-emissions it gets on every roster
    // mutation — a key that flips on each one is a diff nobody can read.
    let meta = metadata(worker());

    assert!(meta.get("control").is_none());
    assert!(meta.get("controlReason").is_none());
    assert!(meta.get("controlSince").is_none());
    // The pre-existing keys are untouched, so a backend that has never heard of
    // handoff reads exactly what it read before.
    assert_eq!(meta["address"], "GRVaddr");
    assert_eq!(meta["workspace"], "/repos/acme");
}

#[test]
fn an_operator_held_harness_advertises_the_hold_with_its_reason() {
    let meta = metadata(HubWorker {
        control: HandoffControl::Operator,
        control_reason: Some("pairing on the auth migration".to_string()),
        control_since: Some(1_753_420_000_000),
        ..worker()
    });

    // "operator", not "user": medulla-v1 reasons about operators, and one word
    // per concept across the two repos is worth more than matching the local
    // enum's variant name.
    assert_eq!(meta["control"], "operator");
    assert_eq!(meta["controlReason"], "pairing on the auth migration");
    assert_eq!(meta["controlSince"], 1_753_420_000_000i64);
}

#[test]
fn a_hold_with_no_reason_omits_the_key_rather_than_sending_blank() {
    let meta = metadata(HubWorker {
        control: HandoffControl::Operator,
        control_reason: Some("   ".to_string()),
        ..worker()
    });

    assert_eq!(meta["control"], "operator");
    assert!(
        meta.get("controlReason").is_none(),
        "whitespace is not a reason"
    );
}

#[test]
fn a_handed_back_harness_carries_its_brief() {
    let meta = metadata(HubWorker {
        handoff: Some(brief()),
        ..worker()
    });

    assert!(meta.get("control").is_none(), "the orchestrator holds it");
    assert_eq!(meta["handoff"]["id"], "w_3-1");
    assert_eq!(meta["handoff"]["workspacePath"], "/repos/acme");
    assert_eq!(meta["handoff"]["branch"], "feat/login");
    assert_eq!(meta["handoff"]["note"], "stuck on the failing e2e");
    assert_eq!(meta["handoff"]["transcriptTruncated"], false);
}

#[test]
fn a_brief_on_a_re_taken_harness_is_not_advertised() {
    // The stale-invitation case. A brief says "continue this work here"; on a
    // harness the operator has since taken back, acting on it is refused. Left
    // advertised it would cost the orchestrator a planning pass every cycle.
    let meta = metadata(HubWorker {
        control: HandoffControl::Operator,
        handoff: Some(brief()),
        ..worker()
    });

    assert_eq!(meta["control"], "operator");
    assert!(
        meta.get("handoff").is_none(),
        "an invitation into a workspace the operator holds is not actionable"
    );
}
