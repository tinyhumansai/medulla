//! An agent's declared workspace, host and capacity, carried into the advert.

use super::super::super::roster::register_payload;
use super::helpers::{no_presence, worker};

/// The declaration → advert chain, at the seam where a declared agent becomes a
/// roster row.
#[test]
fn a_declared_agents_placement_survives_into_the_advert() {
    let spec = crate::hub::WorkerSpec {
        id: "api-claude".to_string(),
        host_id: "this-device".to_string(),
        address: "this-device".to_string(),
        name: "API".to_string(),
        description: "claude on this machine · /srv/api".to_string(),
        harness: "claude".to_string(),
        workspace: Some(crate::runtime::WorkspaceRef::checkout("/srv/api")),
        max_sessions: 1,
    };

    let w = super::super::super::roster::worker_from_spec(&spec);
    assert_eq!(w.id, "api-claude");
    assert_eq!(w.host_id, "this-device");
    assert_eq!(w.label.as_deref(), Some("API"));
    assert_eq!(w.workspace_path(), Some("/srv/api"));
    assert_eq!(w.max_sessions, 1);

    let payload = register_payload(&[w], &no_presence(), &[]);
    let agent = &payload["agents"][0];
    // Placement still rides `metadata.workspace` as a path — the `{path, type}`
    // object is deferred, and the backend reads both anyway.
    assert_eq!(agent["metadata"]["workspace"], "/srv/api");
    // A workspace-backed agent carries NO `hostId`. The library's contract for
    // `AgentDescriptor.hostId` is that it names the host a *local* agent runs
    // on and "must NEVER be set on a harness-backed agent", whose host is
    // derived by walking up from its workspace. Emitting it here made the
    // server take its `a supplied workspaceId or hostId always wins` early
    // return and skip synthesizing a `workspaceId` from `metadata.workspace` —
    // which orphaned every agent from the agent→workspace→harness→host chain:
    // `host_list` still rendered them, but placement answered "no agent inside
    // <host> is available (none declared there)" and nothing could dispatch.
    // The host still reaches the wire, once, in the `hosts[]` block.
    assert!(agent.get("hostId").is_none());
    assert!(agent["metadata"].get("hostId").is_none());
    assert_eq!(payload["hosts"][0]["hostId"], "this-device");
    assert_eq!(agent["metadata"]["maxSessions"], 1);
}

/// The mirror of the rule above: an agent with no workspace has nothing to walk
/// up from, so `hostId` is the only thing that can place it — and it is exactly
/// the case the library reserves the field for.
#[test]
fn a_workspaceless_agent_still_carries_its_host_id() {
    let spec = crate::hub::WorkerSpec {
        id: "this-device-codex".to_string(),
        host_id: "this-device".to_string(),
        address: "this-device".to_string(),
        name: "codex".to_string(),
        description: "codex on this machine".to_string(),
        harness: "codex".to_string(),
        workspace: None,
        max_sessions: 1,
    };
    let w = super::super::super::roster::worker_from_spec(&spec);
    let payload = register_payload(&[w], &no_presence(), &[]);
    let agent = &payload["agents"][0];
    assert!(agent["metadata"].get("workspace").is_none());
    assert_eq!(agent["hostId"], "this-device");
}

/// A remembered roster row and an env-seeded one state no capacity at all.
/// Reading that as zero would tell placement the agent is saturated, which is
/// the opposite of the permissive default every other unstated field takes.
#[test]
fn a_spec_that_states_no_capacity_falls_back_to_the_serial_default() {
    let spec = crate::hub::WorkerSpec {
        id: "remote".to_string(),
        address: "GRVaddr".to_string(),
        name: "medulla-worker".to_string(),
        harness: "claude".to_string(),
        ..Default::default()
    };

    let w = super::super::super::roster::worker_from_spec(&spec);
    assert_eq!(w.max_sessions, 1);
    assert_eq!(w.host_id, "", "this hub does not claim to know");
    assert_eq!(
        w.label, None,
        "the placeholder name is not a label the operator chose"
    );
    assert_eq!(w.workspace_path(), None);
}

/// Which lane a dispatch's task is filed under. Resolving by address alone was
/// right while a machine was one entry; with several agents sharing an address
/// it files every task under whichever is listed first.
#[test]
fn a_task_is_grouped_under_the_agent_it_named_not_the_first_at_that_address() {
    let mut claude = worker("this-device", "this-device");
    claude.workspace = Some(crate::runtime::WorkspaceRef::checkout("/srv/api"));
    let mut codex = worker("this-device-codex", "this-device");
    codex.harness = "codex".to_string();
    let workers = [claude, codex];

    assert_eq!(
        super::super::super::roster::lane_id(&workers, "this-device-codex", Some("this-device")),
        "this-device-codex"
    );
    // An unattributed dispatch has no id to prefer, so the machine's first agent
    // is the honest answer.
    assert_eq!(
        super::super::super::roster::lane_id(&workers, "", Some("this-device")),
        "this-device"
    );
    // A worker addressed by its cryptoId still resolves to its id.
    let remote = [worker("alpha", "GRVaddr")];
    assert_eq!(
        super::super::super::roster::lane_id(&remote, "GRVaddr", Some("GRVaddr")),
        "alpha"
    );
    assert_eq!(
        super::super::super::roster::lane_id(&[], "nobody", None),
        ""
    );
}
