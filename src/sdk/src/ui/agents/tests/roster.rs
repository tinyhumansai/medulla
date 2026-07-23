//! Unit tests for the worker-registry → roster merge.
//!
//! The merge is the only thing that puts a locally-added worker on the Agents
//! tab, and the ways it can go wrong are both silent: drop the worker and it
//! stays invisible while running tasks; fail to recognise it as an already-known
//! peer and one worker renders as two lanes.

use serde_json::{json, Map};

use crate::runtime::{AgentDescriptor, WorkerInfo};
use crate::ui::agents::{merge_worker_roster, worker_descriptor};

/// A registry worker at `address`, identified by `id`.
fn worker(id: &str, address: &str) -> WorkerInfo {
    WorkerInfo {
        id: id.into(),
        address: address.into(),
        handle: None,
        label: None,
        harness: Some("claude".into()),
        peer_id: None,
        selected: false,
    }
}

/// A backend-advertised descriptor for `id`, optionally carrying an address.
fn descriptor(id: &str, address: Option<&str>) -> AgentDescriptor {
    let mut metadata = Map::new();
    if let Some(a) = address {
        metadata.insert("address".into(), json!(a));
    }
    AgentDescriptor {
        id: id.into(),
        name: id.into(),
        metadata,
        ..Default::default()
    }
}

#[test]
fn a_registry_worker_absent_from_the_roster_is_appended() {
    let merged = merge_worker_roster(&[], &[worker("w-1", "addr-1")]);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].id, "w-1");
    // The harness metadata is what tags the lane label downstream.
    assert_eq!(
        merged[0].metadata.get("harness").and_then(|v| v.as_str()),
        Some("claude")
    );
    // Liveness is not claimed: the registry knows reachability, not presence.
    assert_eq!(merged[0].availability, "");
}

#[test]
fn the_backend_roster_wins_and_is_never_duplicated() {
    let roster = vec![descriptor("w-1", None)];
    let merged = merge_worker_roster(&roster, &[worker("w-1", "addr-1")]);
    assert_eq!(merged.len(), 1);
    // The advertised descriptor survives untouched.
    assert_eq!(merged[0], roster[0]);
}

#[test]
fn one_peer_under_two_names_is_still_one_lane() {
    // A pre-seeded `MEDULLA_HUB_WORKERS="alpha=addr-1"` names the peer `alpha`;
    // the backend advertises it by address. Same destination, one lane.
    let roster = vec![descriptor("addr-1", None)];
    assert_eq!(
        merge_worker_roster(&roster, &[worker("alpha", "addr-1")]).len(),
        1
    );
    // And the mirror image: matched through the descriptor's address metadata.
    let roster = vec![descriptor("dev-1", Some("addr-1"))];
    assert_eq!(
        merge_worker_roster(&roster, &[worker("addr-1", "addr-1")]).len(),
        1
    );
}

#[test]
fn two_addressless_entries_are_not_assumed_to_be_the_same_peer() {
    // Blank must never match blank, or the first unaddressed worker would
    // swallow every other one.
    let roster = vec![descriptor("dev-1", None)];
    let merged = merge_worker_roster(&roster, &[worker("w-1", "")]);
    assert_eq!(merged.len(), 2);
}

#[test]
fn the_name_prefers_the_operators_label_then_the_handle_then_the_address() {
    let mut w = worker("w-1", "addr-1");
    assert_eq!(worker_descriptor(&w).name, "addr-1");
    w.handle = Some("@build-box".into());
    assert_eq!(worker_descriptor(&w).name, "@build-box");
    w.label = Some("build box".into());
    assert_eq!(worker_descriptor(&w).name, "build box");
    // A blank label is not a name; the next-best identifier stands.
    w.label = Some("   ".into());
    assert_eq!(worker_descriptor(&w).name, "@build-box");
}

#[test]
fn a_harnessless_worker_carries_no_harness_metadata() {
    // An empty harness must not tag the lane with an empty `[]` prefix.
    let mut w = worker("w-1", "addr-1");
    w.harness = Some(String::new());
    let d = worker_descriptor(&w);
    assert!(d.metadata.get("harness").is_none());
    assert_eq!(d.description, "");
}
