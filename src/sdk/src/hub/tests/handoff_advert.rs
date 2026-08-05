//! What the roster advert says about who holds a session: **nothing**.
//!
//! It used to say a great deal — `control`, `controlReason`, `controlSince`, and
//! the handback brief — and every one of those keys was right when a worker *was*
//! an agent *was* a machine *was* one implicit session. An agent now runs N
//! sessions, a person takes *one*, and none of those keys has anywhere to put
//! which one. A backend folding held-state onto its ledger by `agentId` would
//! therefore mark every pending task on that agent as held, including the ones
//! running fine in sibling sessions.
//!
//! So these tests pin the *absence*. They are written as absence assertions
//! rather than deleted outright because the keys were once present and correct,
//! and a future reader looking at [`HubWorker::control`] — which still exists,
//! and still drives medulla's own dispatch — will otherwise reasonably conclude
//! that not advertising it is an oversight. It is not: saying it correctly needs
//! session grain on the wire, which arrives with inbound session targeting (§C3).
//!
//! The local behaviour these keys used to describe is unchanged and covered
//! elsewhere: dispatch skips a session an operator holds, an in-flight turn
//! suspends instead of being discarded, and the hand-back turn delivers the
//! task's result through the ordinary result frame.

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
        workspace: Some(crate::runtime::WorkspaceRef::checkout("/repos/acme")),
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
    register_payload(&[w], &no_presence(), &[], &[])["agents"][0]["metadata"].clone()
}

#[test]
fn an_orchestrator_held_harness_says_nothing_about_control() {
    let meta = metadata(worker());

    assert!(meta.get("control").is_none());
    assert!(meta.get("controlReason").is_none());
    assert!(meta.get("controlSince").is_none());
    // The keys that do place the agent are untouched.
    assert_eq!(meta["address"], "GRVaddr");
    assert_eq!(meta["workspace"], "/repos/acme");
}

#[test]
fn an_operator_held_agent_advertises_exactly_what_an_unheld_one_does() {
    // The grain mismatch, stated as a test. Control is per-agent here and
    // per-session in the model, so "held" on this advert cannot say *which* of
    // the agent's sessions a person took — and a backend keying held-state by
    // `agentId` would apply it to every task the agent is running.
    //
    // Do not "restore" these keys. Advertising a hold needs the session id the
    // hold is about, which needs inbound session targeting (§C3).
    let held = metadata(HubWorker {
        control: HandoffControl::Operator,
        control_reason: Some("pairing on the auth migration".to_string()),
        control_since: Some(1_753_420_000_000),
        ..worker()
    });

    assert!(held.get("control").is_none(), "a hold is local state");
    assert!(held.get("controlReason").is_none());
    assert!(held.get("controlSince").is_none());
    assert_eq!(
        held,
        metadata(worker()),
        "taking a session must not change one byte of the agent's advert"
    );
}

#[test]
fn a_handed_back_harness_does_not_carry_its_brief() {
    // The brief is per *session* — it names one (`sessionId`, and a transcript
    // from that one pty) — but this slot is per *agent*, so two sessions handed
    // back on one agent silently overwrite each other and the reader cannot tell
    // that happened. It was also emitted through the same per-agent control gate
    // as the keys above, which means whether an operator saw a brief at all
    // depended on whether some *unrelated* session of that agent was held.
    //
    // With control off the wire this would be the last piece of agent-grain
    // control state left on it: a brief exists only because a person took a
    // session and gave it back. It travels again when a brief can name its
    // session on the wire (§C3).
    let meta = metadata(HubWorker {
        handoff: Some(brief()),
        ..worker()
    });

    assert!(meta.get("handoff").is_none());
    assert_eq!(
        meta,
        metadata(worker()),
        "a handback must not change the agent's advert either"
    );
}

#[test]
fn the_local_hold_state_survives_even_though_it_is_never_advertised() {
    // The other half of the change, and the reason `control` is still a field:
    // medulla's own dispatch reads it. Dropping the advert keys must not be
    // mistaken for dropping the feature — the roster still knows, it just does
    // not tell the backend something the backend cannot key correctly.
    let held = HubWorker {
        control: HandoffControl::Operator,
        control_reason: Some("pairing on the auth migration".to_string()),
        control_since: Some(1_753_420_000_000),
        handoff: Some(brief()),
        ..worker()
    };

    assert!(held.control.is_operator());
    assert_eq!(
        held.control_reason.as_deref(),
        Some("pairing on the auth migration")
    );
    assert_eq!(held.control_since, Some(1_753_420_000_000));
    assert_eq!(
        held.handoff.as_ref().map(|b| b.session_id.as_str()),
        Some("w_3")
    );
}
