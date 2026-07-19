//! Unit tests for the runtime trait surface's helper types — chiefly
//! [`WorkerOp`](super::WorkerOp) parsing and the snapshot defaults.

use super::*;
use crate::runtime::mock::MockRuntime;

#[test]
fn worker_op_parse_add_classifies_handle_address_and_label() {
    // A leading @ marks a tiny.place handle; the remainder is the label.
    match WorkerOp::parse_add("@alice friendly worker") {
        Some(WorkerOp::Add {
            address,
            handle,
            label,
            harness,
        }) => {
            assert_eq!(address, None);
            assert_eq!(handle.as_deref(), Some("@alice"));
            assert_eq!(label.as_deref(), Some("friendly worker"));
            assert_eq!(harness, None);
        }
        other => panic!("expected Add, got {other:?}"),
    }

    // A non-@ first token is an address; no remainder means no label.
    match WorkerOp::parse_add("  tcp://host:9000  ") {
        Some(WorkerOp::Add {
            address,
            handle,
            label,
            ..
        }) => {
            assert_eq!(address.as_deref(), Some("tcp://host:9000"));
            assert_eq!(handle, None);
            assert_eq!(label, None);
        }
        other => panic!("expected Add, got {other:?}"),
    }

    // Blank input yields nothing.
    assert!(WorkerOp::parse_add("   ").is_none());
    assert!(WorkerOp::parse_add("").is_none());
}

#[test]
fn stream_state_glyph_and_label() {
    assert_eq!(StreamState::Live.glyph(), '●');
    assert_eq!(StreamState::Resyncing.glyph(), '◌');
    assert_eq!(StreamState::Stalled.glyph(), '✕');
    assert_eq!(StreamState::Live.label(), "live");
    assert_eq!(StreamState::Resyncing.label(), "resyncing");
    assert_eq!(StreamState::Stalled.label(), "stalled");
}

#[test]
fn stream_state_is_copy_and_eq() {
    let a = StreamState::Live;
    let b = a; // Copy
    assert_eq!(a, b);
    assert_ne!(StreamState::Live, StreamState::Stalled);
}

/// The trait's default methods are exercised through `MockRuntime`, which does
/// not override any of them: they are the no-op fleet/steering seams.
#[tokio::test]
async fn default_trait_methods_are_no_ops() {
    let rt = MockRuntime::empty();
    // Fire-and-forget defaults: must not panic.
    rt.answer_question("cyc-1".into(), "q1".into(), "yes".into());
    rt.cancel_task("cyc-1".into(), "t1".into());
    // Default worker surface is empty and mutations succeed silently.
    assert!(rt.workers().is_empty());
    rt.worker_op(WorkerOp::Select { id: "w1".into() })
        .await
        .unwrap();
    rt.worker_op(WorkerOp::Add {
        address: Some("host:1".into()),
        handle: None,
        label: Some("lbl".into()),
        harness: None,
    })
    .await
    .unwrap();
    rt.worker_op(WorkerOp::Update {
        id: "w1".into(),
        patch: Map::new(),
    })
    .await
    .unwrap();
    rt.worker_op(WorkerOp::Remove { id: "w1".into() })
        .await
        .unwrap();
    // No lossy stream to surface.
    assert!(rt.stream_state().is_none());
}

#[test]
fn agent_descriptor_serde_defaults() {
    // Only `id` is required; every other field defaults.
    let a: AgentDescriptor = serde_json::from_str(r#"{"id":"dev"}"#).unwrap();
    assert_eq!(a.id, "dev");
    assert!(a.name.is_empty());
    assert!(a.tags.is_empty());
    assert!(a.metadata.is_empty());
    let round: AgentDescriptor =
        serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
    assert_eq!(a, round);
}

#[test]
fn value_types_are_debug_clone_eq() {
    let presence = AgentPresence {
        online: true,
        detail: Some("idle".into()),
        at: 5,
    };
    assert_eq!(presence.clone(), presence);
    assert!(format!("{presence:?}").contains("AgentPresence"));

    let peer = PeerSession {
        id: "s1".into(),
        state: "idle".into(),
        harness: None,
        last_seen_at: 1,
    };
    assert_eq!(peer.clone(), peer);

    let thread = ThreadSummary {
        id: "t1".into(),
        parent_id: None,
        name: "main".into(),
        running: false,
        turns: 2,
        running_tasks: 0,
        attention: 0,
    };
    assert_eq!(thread.clone(), thread);

    let ident = TinyplaceIdentity {
        agent_id: "a".into(),
        public_key: "pk".into(),
        handle: Some("@h".into()),
    };
    assert_eq!(ident.clone(), ident);

    let worker = WorkerInfo {
        id: "w1".into(),
        address: "host".into(),
        handle: None,
        label: None,
        harness: None,
        peer_id: None,
        selected: false,
    };
    assert_eq!(worker.clone(), worker);

    let ctx = ContextItem {
        ref_: "r".into(),
        kind: "memory".into(),
        bytes: 3,
        content: "c".into(),
    };
    assert_eq!(ctx.clone(), ctx);
}

#[test]
fn cycle_result_summary_default_is_empty() {
    let s = CycleResultSummary::default();
    assert_eq!(s.pass_count, 0);
    assert!(s.task_ledger.is_empty());
}

#[test]
fn worker_op_is_debug_clone() {
    let op = WorkerOp::Add {
        address: Some("h".into()),
        handle: Some("@a".into()),
        label: None,
        harness: Some("codex".into()),
    };
    let cloned = op.clone();
    assert!(format!("{cloned:?}").contains("Add"));
}
