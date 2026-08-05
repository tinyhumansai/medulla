//! Unit tests for the `Host → Agents` tree.

use super::{host_rows, HostKind};
use crate::config::LocalHostRef;
use crate::runtime::{AgentDeclaration, WorkerInfo};

/// A roster entry with nothing probed — the shape an added peer has.
fn worker(id: &str, address: &str) -> WorkerInfo {
    WorkerInfo {
        id: id.into(),
        address: address.into(),
        handle: None,
        label: None,
        harness: Some("codex".into()),
        workspace: None,
        peer_id: None,
        cpu_cores: None,
        memory_total_bytes: None,
        memory_available_bytes: None,
        ip_address: None,
        selected: false,
        roles: Vec::new(),
        budgets: Vec::new(),
        readiness: Vec::new(),
    }
}

/// The device-local host, as `local_hosts` resolves it.
fn this_device() -> LocalHostRef {
    LocalHostRef {
        id: "this-device".into(),
        name: "this device".into(),
        workspace: "/w".into(),
        primary: true,
    }
}

#[test]
fn the_local_host_is_present_with_no_agents_and_no_roster() {
    let rows = host_rows(&[], &[], &[this_device()]);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "this-device");
    assert_eq!(rows[0].kind, HostKind::Local);
    assert!(rows[0].agents.is_empty());
    assert!(rows[0].accepts_new_agents());
    assert!(rows[0].detail_worker.is_none());
}

#[test]
fn declared_agents_hang_off_their_host_whether_or_not_they_are_running() {
    let mut running =
        AgentDeclaration::new("medulla-claude", "this-device", "claude", "/w/medulla");
    running.roles = vec!["coder".into()];
    let idle = AgentDeclaration::new("api-codex", "this-device", "codex", "/w/api");

    let rows = host_rows(
        &[worker("medulla-claude", "this-device")],
        &[running, idle],
        &[this_device()],
    );

    assert_eq!(rows.len(), 1, "one machine, one host row");
    let agents = &rows[0].agents;
    assert_eq!(agents.len(), 2);
    assert_eq!(agents[0].agent_id, "medulla-claude");
    assert_eq!(agents[0].harness.as_deref(), Some("claude"));
    assert_eq!(agents[0].workspace.as_deref(), Some("/w/medulla"));
    assert_eq!(agents[0].roles, vec!["coder".to_string()]);
    assert_eq!(agents[0].max_sessions, Some(1), "checkout ⇒ serial");
    assert!(agents[0].declared && agents[0].editable && agents[0].live);
    // Declared but with nothing in the roster: still an agent, not yet running.
    assert!(agents[1].declared && agents[1].editable && !agents[1].live);
}

#[test]
fn a_roster_entry_no_declaration_covers_is_still_listed_and_still_editable() {
    // The migration seed: an install that predates declarations advertises
    // agents nobody wrote down. They are this machine's, so a role can be
    // assigned — which is what writes the declaration.
    let rows = host_rows(
        &[worker("this-device", "this-device")],
        &[],
        &[this_device()],
    );
    let agent = &rows[0].agents[0];
    assert_eq!(agent.agent_id, "this-device");
    assert!(!agent.declared, "no declaration covers it");
    assert!(agent.editable, "but it is on this machine");
    assert!(agent.live);
    assert_eq!(agent.max_sessions, None, "nothing declared a strategy");
}

#[test]
fn remote_peers_group_under_their_address_and_are_read_only() {
    let mut labelled = worker("build-box", "7Kx9fQ");
    labelled.label = Some("build box".into());
    let sibling = worker("build-box-codex", "7Kx9fQ");

    let rows = host_rows(
        &[labelled, sibling, worker("other", "9zzz")],
        &[],
        &[this_device()],
    );

    assert_eq!(rows.len(), 3, "local + two remotes");
    assert_eq!(rows[1].id, "7Kx9fQ");
    assert_eq!(rows[1].label, "build box");
    assert_eq!(rows[1].kind, HostKind::Remote);
    assert!(!rows[1].accepts_new_agents());
    assert_eq!(rows[1].agents.len(), 2, "both entries at that address");
    for agent in &rows[1].agents {
        assert!(!agent.declared, "a remote's declarations live over there");
        assert!(!agent.editable, "and are not editable from here");
        assert!(agent.live);
    }
    assert_eq!(rows[2].id, "9zzz");
    assert_eq!(
        rows[2].label, "9zzz",
        "no label, no handle: the raw address"
    );
}

#[test]
fn one_peer_registered_twice_is_one_agent() {
    // The same peer reached the registry from two directions — added by hand and
    // advertised. Listing it twice would read as two machines' worth of capacity.
    let rows = host_rows(
        &[worker("addr-1", "addr-1"), worker("addr-1", "addr-1")],
        &[],
        &[this_device()],
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].agents.len(), 1, "{rows:?}");
}

#[test]
fn the_preview_reads_capacity_from_whichever_entry_probed_the_machine() {
    // Capacity belongs to the machine, so the host row points at the entry that
    // reported it rather than at whichever agent happens to be first.
    let bare = worker("agent-a", "this-device");
    let mut probed = worker("agent-b", "this-device");
    probed.cpu_cores = Some(8);

    let rows = host_rows(&[bare, probed], &[], &[this_device()]);
    assert_eq!(rows[0].detail_worker.as_deref(), Some("agent-b"));
}

#[test]
fn a_remote_hosts_preview_reads_capacity_from_its_probed_entry_too() {
    // The remote branch used to keep whichever agent opened the row, so a
    // machine whose first agent had never been probed read "not reported" while
    // a sibling at the same address held the capacity.
    let bare = worker("agent-a", "addr-1");
    let mut probed = worker("agent-b", "addr-1");
    probed.cpu_cores = Some(8);

    let rows = host_rows(&[bare, probed.clone()], &[], &[this_device()]);
    let remote = rows
        .iter()
        .find(|row| row.kind == HostKind::Remote)
        .expect("the peer is a remote host");
    assert_eq!(remote.agents.len(), 2, "both agents are listed: {remote:?}");
    assert_eq!(remote.detail_worker.as_deref(), Some("agent-b"));

    // And a probed pick is not given up for a later unprobed one.
    let rows = host_rows(
        &[probed, worker("agent-c", "addr-1")],
        &[],
        &[this_device()],
    );
    let remote = rows
        .iter()
        .find(|row| row.kind == HostKind::Remote)
        .expect("the peer is a remote host");
    assert_eq!(remote.detail_worker.as_deref(), Some("agent-b"));
}

#[test]
fn a_declaration_naming_an_unconfigured_host_still_gets_a_local_row() {
    // The `[[hosts]]` entry was removed, or the id was hand-written. The agents
    // are still declared on this machine — hiding them would lose them.
    let orphan = AgentDeclaration::new("ghost", "local-gone", "claude", "/w/gone");
    let rows = host_rows(&[], &[orphan], &[this_device()]);

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].id, "local-gone");
    assert_eq!(rows[1].kind, HostKind::Local);
    assert_eq!(rows[1].agents[0].agent_id, "ghost");
    assert!(rows[1].agents[0].editable);
}

#[test]
fn an_agent_that_names_no_host_belongs_to_the_machine_looking_at_it() {
    // Both tabs draw this tree, and the Agents rail has always drawn an unplaced
    // agent beside the local ones. Dropping it here would mean the two lenses
    // disagree about what exists — and on the Hosts tab it would simply vanish.
    let unplaced = AgentDeclaration::new("drifter", "", "claude", "/w/drifter");
    let rows = host_rows(&[], &[unplaced], &[this_device()]);

    assert_eq!(rows.len(), 1, "no second machine is conjured: {rows:?}");
    assert_eq!(rows[0].agents[0].agent_id, "drifter");
    assert!(rows[0].agents[0].editable);
}

#[test]
fn an_unplaced_agent_survives_a_config_with_no_local_host_at_all() {
    let unplaced = AgentDeclaration::new("drifter", "", "claude", "/w/drifter");
    let rows = host_rows(&[], &[unplaced], &[]);

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].label, "this device");
    assert_eq!(rows[0].kind, HostKind::Local);
    assert_eq!(rows[0].agents[0].agent_id, "drifter");
}

#[test]
fn only_the_first_local_host_claims_the_unplaced() {
    // Two local hosts and one agent naming neither: it must appear once, under
    // the primary, not under every host on the machine.
    let mut second = this_device();
    second.id = "local-api".into();
    second.name = "api".into();
    second.primary = false;
    let unplaced = AgentDeclaration::new("drifter", "", "claude", "/w/drifter");

    let rows = host_rows(&[], &[unplaced], &[this_device(), second]);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].agents.len(), 1);
    assert!(rows[1].agents.is_empty(), "claimed once: {rows:?}");
}

#[test]
fn a_blank_declared_name_falls_back_to_the_agent_id() {
    let mut declared = AgentDeclaration::new("a1", "this-device", "claude", "/w");
    declared.name = Some("   ".into());
    let rows = host_rows(&[], &[declared], &[this_device()]);
    assert_eq!(rows[0].agents[0].label, "a1");
}

#[test]
fn an_agent_named_by_a_declaration_is_never_also_a_remote_host() {
    // The roster entry is registered under a different address than the host id
    // its declaration names. It is still that host's agent; listing it again as
    // a remote host would advertise one agent as two machines.
    let declaration = AgentDeclaration::new("roamer", "this-device", "claude", "/w");
    let rows = host_rows(
        &[worker("roamer", "somewhere-else")],
        &[declaration],
        &[this_device()],
    );

    assert_eq!(rows.len(), 1, "no phantom remote host: {rows:?}");
    assert!(rows[0].agents[0].live);
}

#[test]
fn a_padded_declared_id_still_claims_its_worker_exactly_once() {
    // `agent_id` is typed into a config file, and the declaration→roster claim
    // is trimmed — so a stray space matched the worker there and then failed to
    // recognise it in the undeclared pass, listing one agent twice: once as the
    // declaration, once as a roster entry nobody had written down.
    let declared = AgentDeclaration::new("  padded  ", "this-device", "claude", "/w");
    let rows = host_rows(
        &[worker("padded", "this-device")],
        &[declared],
        &[this_device()],
    );

    assert_eq!(rows.len(), 1, "no phantom second host: {rows:?}");
    assert_eq!(rows[0].agents.len(), 1, "one agent, listed once: {rows:?}");
    let agent = &rows[0].agents[0];
    assert!(agent.declared, "the declaration is what describes it");
    assert!(agent.live, "and the roster entry is folded into it");
}

#[test]
fn the_declared_name_and_roles_win_over_the_live_entry() {
    // The declaration is what survives a restart, so it is what the row shows —
    // a live entry whose roles were set but never written down must not read as
    // the persisted answer.
    let mut declared = AgentDeclaration::new("a1", "this-device", "claude", "/w");
    declared.name = Some("the good one".into());
    declared.roles = vec!["reviewer".into()];
    let mut live = worker("a1", "this-device");
    live.label = Some("stale label".into());
    live.roles = vec!["coder".into()];
    live.selected = true;

    let rows = host_rows(&[live], &[declared], &[this_device()]);
    let agent = &rows[0].agents[0];
    assert_eq!(agent.label, "the good one");
    assert_eq!(agent.roles, vec!["reviewer".to_string()]);
    assert!(agent.selected, "selection is live state, and is kept");
}
