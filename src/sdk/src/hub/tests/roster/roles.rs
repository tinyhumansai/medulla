//! An agent's declared roles, workspace and capacity, carried into the advert.

use super::super::super::roster::register_payload;
use super::helpers::{no_presence, worker};

#[test]
fn a_toggled_role_reaches_the_orchestrator_as_description_tags_and_id() {
    // What the orchestrator can route on. The description is a prompt surface —
    // it is the text the model reads when choosing — the tags are the coarse
    // filter, and the id is the join key for anything that later wants the
    // role's tools or instructions.
    let catalog = crate::agents::default_templates();
    let mut w = worker("claude-worker-2", "GRVaddr");
    w.roles = vec!["code-reviewer".to_string()];

    let payload = register_payload(&[w], &no_presence(), &catalog, &[]);
    let agent = &payload["agents"][0];

    assert!(
        agent["description"]
            .as_str()
            .expect("a description")
            .contains("Reviews a change"),
        "the role's own words, not \"claude daemon\": {agent}"
    );
    let tags: Vec<&str> = agent["tags"]
        .as_array()
        .expect("tags")
        .iter()
        .map(|t| t.as_str().expect("a tag"))
        .collect();
    // `code` survives whatever else is set: every one of these runs a coding
    // harness, and dropping it would take the worker out of code fan-outs.
    assert!(tags.contains(&"code"), "{tags:?}");
    assert!(tags.contains(&"review"), "{tags:?}");
    assert_eq!(agent["metadata"]["roles"][0], "code-reviewer");
}

#[test]
fn a_worker_with_no_roles_is_advertised_exactly_as_before() {
    // Unspecified must stay general. Describing a worker as nothing, or tagging
    // it out of every fan-out, would make declaring roles compulsory.
    let payload = register_payload(
        &[worker("w1", "GRVaddr")],
        &no_presence(),
        &crate::agents::default_templates(),
        &[],
    );
    let agent = &payload["agents"][0];
    assert_eq!(agent["description"], "claude daemon");
    assert_eq!(agent["tags"][0], "code");
    assert_eq!(agent["tags"].as_array().expect("tags").len(), 1);
}

#[test]
fn a_role_the_catalog_does_not_have_is_dropped_rather_than_advertised() {
    // A role the orchestrator cannot look up is a routing hint it cannot act
    // on, and advertising it would describe the worker as something no template
    // backs.
    let mut w = worker("w1", "GRVaddr");
    w.roles = vec!["deleted-role".to_string()];
    let payload = register_payload(
        &[w],
        &no_presence(),
        &crate::agents::default_templates(),
        &[],
    );
    assert_eq!(payload["agents"][0]["description"], "claude daemon");
    // Including from `metadata.roles`, which is the join key: an id with no
    // template behind it hands a downstream lookup a key that resolves to
    // nothing, which is the same unactionable hint the description drops.
    assert!(
        payload["agents"][0]["metadata"]["roles"].is_null(),
        "unresolved ids must not survive in metadata: {}",
        payload["agents"][0]["metadata"]
    );
}

#[test]
fn metadata_roles_carries_only_the_ids_the_catalog_resolves() {
    let mut w = worker("w1", "GRVaddr");
    w.roles = vec!["code-reviewer".to_string(), "deleted-role".to_string()];
    let payload = register_payload(
        &[w],
        &no_presence(),
        &crate::agents::default_templates(),
        &[],
    );
    assert_eq!(
        payload["agents"][0]["metadata"]["roles"],
        serde_json::json!(["code-reviewer"])
    );
}

/// The declaration → advert chain, at the seam where a declared agent becomes a
/// roster row. Roles had no source here before agents were declared: the
/// mapping hard-coded an empty list, so a host in this process advertised itself
/// as a general worker however the operator had configured it.
#[test]
fn a_declared_agents_roles_and_placement_survive_into_the_advert() {
    let spec = crate::hub::WorkerSpec {
        id: "api-claude".to_string(),
        host_id: "this-device".to_string(),
        address: "this-device".to_string(),
        name: "API".to_string(),
        description: "claude on this machine · /srv/api".to_string(),
        harness: "claude".to_string(),
        workspace: Some(crate::runtime::WorkspaceRef::checkout("/srv/api")),
        roles: vec!["code-reviewer".to_string()],
        max_sessions: 1,
    };

    let w = super::super::super::roster::worker_from_spec(&spec);
    assert_eq!(w.id, "api-claude");
    assert_eq!(w.host_id, "this-device");
    assert_eq!(w.label.as_deref(), Some("API"));
    assert_eq!(w.workspace_path(), Some("/srv/api"));
    assert_eq!(w.max_sessions, 1);

    let payload = register_payload(
        &[w],
        &no_presence(),
        &crate::agents::default_templates(),
        &[],
    );
    let agent = &payload["agents"][0];
    assert_eq!(agent["metadata"]["roles"][0], "code-reviewer");
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
        roles: vec![],
        max_sessions: 1,
    };
    let w = super::super::super::roster::worker_from_spec(&spec);
    let payload = register_payload(
        &[w],
        &no_presence(),
        &crate::agents::default_templates(),
        &[],
    );
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
