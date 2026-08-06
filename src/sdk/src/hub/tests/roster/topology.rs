//! The host block: five machines behind one hub socket must read as five hosts,
//! not as one synthesized `host:${socketId}`.

use super::super::super::roster::register_payload;
use super::helpers::{declared, no_presence, placed, worker};

/// The whole point of the block: five machines behind one hub socket must read
/// as five hosts, not as one synthesized `host:${socketId}`.
#[test]
fn the_advert_names_every_host_its_agents_run_on() {
    let workers = [
        placed("this-device", "this-device"),
        placed("this-device-codex", "this-device"),
        placed("api", "local-backend"),
    ];
    let declared_hosts = [
        declared("this-device", "this device"),
        declared("local-backend", "backend"),
    ];

    let payload = register_payload(&workers, &no_presence(), &[], &declared_hosts);
    let hosts = payload["hosts"].as_array().expect("a hosts block");

    assert_eq!(
        hosts.len(),
        2,
        "one entry per host, not per agent: {hosts:?}"
    );
    assert_eq!(hosts[0]["hostId"], "this-device");
    assert_eq!(hosts[0]["name"], "this device");
    assert_eq!(hosts[0]["kind"], "local");
    assert_eq!(hosts[0]["address"], "this-device");
    assert_eq!(hosts[1]["hostId"], "local-backend");
    assert_eq!(hosts[1]["name"], "backend");
    assert_eq!(hosts[1]["kind"], "local");

    // And every agent says which of them it runs on, which is what makes the
    // backend prefer these ids over its own synthesis.
    let agents = payload["agents"].as_array().expect("agents");
    assert_eq!(agents[0]["hostId"], "this-device");
    assert_eq!(agents[1]["hostId"], "this-device");
    assert_eq!(agents[2]["hostId"], "local-backend");
}

/// `kind` is decided by the declaration and nothing else — a host this machine
/// declares is local, and any other host an agent names is one this hub merely
/// fronts. Nothing is probed to establish it.
#[test]
fn a_host_this_machine_did_not_declare_is_advertised_as_remote() {
    let workers = [
        placed("mine", "this-device"),
        placed("theirs", "mac-studio"),
    ];
    let payload = register_payload(
        &workers,
        &no_presence(),
        &[],
        &[declared("this-device", "this device")],
    );
    let hosts = payload["hosts"].as_array().expect("a hosts block");

    assert_eq!(hosts[0]["kind"], "local");
    assert_eq!(hosts[1]["hostId"], "mac-studio");
    assert_eq!(hosts[1]["kind"], "remote");
    assert!(
        hosts[1].get("name").is_none(),
        "a host learned from a placement has an id and nothing to call it"
    );
    assert_eq!(hosts[1]["address"], "mac-studio");
}

/// The peer an operator added by address: this hub has no idea which machine it
/// is. Synthesizing a host for it would invent a fact, so the key is omitted and
/// the backend's own `host:${socketId}` fallback still applies to exactly those.
#[test]
fn an_unplaced_worker_carries_no_host_and_creates_none() {
    let payload = register_payload(&[worker("peer", "GRVaddr")], &no_presence(), &[], &[]);

    assert!(
        payload.get("hosts").is_none(),
        "an empty block is a key that says nothing: {payload}"
    );
    assert!(payload["agents"][0].get("hostId").is_none());
}

/// The two halves of one payload must agree: a host whose agents were all
/// withheld by the liveness filter is withheld too, so no entry ever describes a
/// host with nothing on it.
#[test]
fn a_host_whose_agents_are_all_offline_is_withheld_with_them() {
    let workers = [placed("live", "this-device"), placed("dead", "mac-studio")];
    let online = std::collections::HashMap::from([("mac-studio".to_string(), false)]);
    let payload = register_payload(
        &workers,
        &online,
        &[],
        &[declared("this-device", "this device")],
    );

    let hosts = payload["hosts"].as_array().expect("a hosts block");
    assert_eq!(hosts.len(), 1, "{hosts:?}");
    assert_eq!(hosts[0]["hostId"], "this-device");
    assert_eq!(payload["agents"].as_array().expect("agents").len(), 1);
}

/// Never synthesised. The hub holds per-*worker* capability probes, not
/// host-level facts, and aggregating those into a resource claim would be
/// inventing a number for a model whose whole doctrine is "declared, never
/// probed".
#[test]
fn a_host_advertises_no_resources_it_did_not_measure() {
    let payload = register_payload(
        &[placed("mine", "this-device")],
        &no_presence(),
        &[],
        &[declared("this-device", "this device")],
    );

    assert!(payload["hosts"][0].get("resources").is_none());
}

/// The per-agent concurrency contract. Deterministic placement reads it to
/// decide whether an agent has headroom; without it the library's demotion never
/// engages.
#[test]
fn an_agent_advertises_the_sessions_it_may_run_at_once() {
    let payload = register_payload(&[worker("w1", "GRVaddr")], &no_presence(), &[], &[]);
    assert_eq!(
        payload["agents"][0]["metadata"]["maxSessions"], 1,
        "the serial checkout default"
    );

    let mut parallel = worker("w2", "ADDR2");
    parallel.max_sessions = 4;
    let payload = register_payload(&[parallel], &no_presence(), &[], &[]);
    assert_eq!(payload["agents"][0]["metadata"]["maxSessions"], 4);

    // Zero is withheld rather than sent: a capacity of nothing reads as
    // saturated, which is the opposite of what every other omission here means.
    let mut unstated = worker("w3", "ADDR3");
    unstated.max_sessions = 0;
    let payload = register_payload(&[unstated], &no_presence(), &[], &[]);
    assert!(payload["agents"][0]["metadata"]
        .get("maxSessions")
        .is_none());
}

/// A worker nobody placed still advertises cleanly — no workspace key, no host
/// key, and everything the backend already read left where it was.
#[test]
fn an_agent_with_no_declared_workspace_still_serializes() {
    let payload = register_payload(&[worker("peer", "GRVaddr")], &no_presence(), &[], &[]);
    let agent = &payload["agents"][0];

    assert!(agent["metadata"].get("workspace").is_none());
    assert!(agent.get("hostId").is_none());
    assert_eq!(agent["id"], "peer");
    assert_eq!(agent["availability"], "online");
    assert_eq!(agent["metadata"]["address"], "GRVaddr");
    assert_eq!(agent["metadata"]["harness"], "claude");
}

/// The whole metadata object, pinned — including what is **not** in it.
///
/// Control state (`control`, `controlReason`, `controlSince`) and the handback
/// brief are per-*agent* keys describing a per-*session* fact, so a backend
/// folding them by `agentId` would mark every task on the agent as held when a
/// person took one session. They are therefore not advertised at all; see
/// `hub::tests::handoff_advert` for the full reasoning and the local state that
/// replaces them. The object is pinned rather than spot-checked because a
/// regression either way is silent — the advert still parses.
#[test]
fn the_advert_metadata_carries_placement_and_never_control() {
    let mut held = placed("this-device", "this-device");
    held.workspace = Some(crate::runtime::WorkspaceRef::checkout("/repos/acme"));
    held.control = super::super::super::HandoffControl::Operator;
    held.control_reason = Some("  pairing on the migration  ".to_string());
    held.control_since = Some(1_753_420_600_000);

    let payload = register_payload(
        &[held],
        &no_presence(),
        &[],
        &[declared("this-device", "this device")],
    );

    assert_eq!(
        payload["agents"][0]["metadata"],
        serde_json::json!({
            "address": "this-device",
            "harness": "claude",
            "maxSessions": 1,
            "workspace": "/repos/acme",
        }),
        "address/harness/maxSessions/workspace must not move, gain a wrapper, or \
         change spelling — and control must not appear at any grain"
    );

    // Nor does the brief a handback produces: it names one session, this slot is
    // one per agent, and it exists only because a person held something.
    let mut handed_back = worker("w1", "GRVaddr");
    handed_back.handoff = Some(super::super::super::HarnessHandoff {
        id: "w_3-1".to_string(),
        at: 1_753_420_600_000,
        session_id: "w_3".to_string(),
        harness_session_id: None,
        provider: "claude".to_string(),
        workspace_path: "/repos/acme".to_string(),
        branch: None,
        project: None,
        note: None,
        transcript: "…pnpm test".to_string(),
        transcript_truncated: false,
    });
    let payload = register_payload(&[handed_back], &no_presence(), &[], &[]);
    assert!(
        payload["agents"][0]["metadata"].get("handoff").is_none(),
        "a per-session brief has no honest home on a per-agent advert"
    );
}
