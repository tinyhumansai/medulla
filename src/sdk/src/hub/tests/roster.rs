//! Tests for the hub roster: how a worker is advertised, addressed, and kept
//! unique.
//!
//! The roster is the only thing standing between an orchestrator's `agentId`
//! and a tiny.place address, so these pin the resolution rules rather than the
//! transport — dispatch itself is covered in [`super::super::dispatch`].

use super::super::roster::{
    address_of, addresses_of, register_payload, unreachable_addresses, HubWorker,
};

/// No liveness opinion — what a bridge with no presence signal reports, and
/// what most of these tests want, since they are about payload shape.
fn no_presence() -> std::collections::HashMap<String, bool> {
    std::collections::HashMap::new()
}
use super::super::roster::{subscription_for_strategy, worker_for_strategy};
use crate::protocol::{
    AgentCapabilities, BudgetSource, BudgetWindow, HarnessBudget, HarnessProvider,
    HarnessReadiness, WorkerSystemInfo,
};
use crate::runtime::{RoutingStrategy, SubscriptionRoutingStrategy};

fn worker(id: &str, addr: &str) -> HubWorker {
    HubWorker {
        roles: Vec::new(),
        id: id.to_string(),
        address: addr.to_string(),
        harness: "claude".to_string(),
        label: None,
        selected: false,
        workspace: None,
        ..Default::default()
    }
}

fn details(cpu: u32, available_gib: u64) -> WorkerSystemInfo {
    WorkerSystemInfo {
        cpu_cores: cpu,
        memory_total_bytes: None,
        memory_available_bytes: Some(available_gib * 1024 * 1024 * 1024),
        ip_address: "10.0.0.1".into(),
    }
}

#[test]
fn capacity_strategies_choose_different_workers() {
    let workers = vec![worker("cpu", "addr-cpu"), worker("ram", "addr-ram")];
    let details = std::collections::HashMap::from([
        ("cpu".into(), details(16, 4)),
        ("ram".into(), details(4, 64)),
    ]);
    assert_eq!(
        worker_for_strategy(&workers, &details, RoutingStrategy::CpuFirst).as_deref(),
        Some("cpu")
    );
    assert_eq!(
        worker_for_strategy(&workers, &details, RoutingStrategy::MemoryFirst).as_deref(),
        Some("ram")
    );
    assert_eq!(
        worker_for_strategy(&workers, &details, RoutingStrategy::Balanced).as_deref(),
        Some("cpu"),
        "balanced routing is CPU-first even when another worker has much more RAM"
    );
    assert_eq!(
        worker_for_strategy(&workers, &details, RoutingStrategy::Manual),
        None
    );
}

#[test]
fn balanced_uses_memory_to_break_cpu_ties_while_cpu_first_does_not() {
    let workers = vec![
        worker("larger", "addr-large"),
        worker("smaller", "addr-small"),
    ];
    let details = std::collections::HashMap::from([
        (
            "smaller".into(),
            WorkerSystemInfo {
                cpu_cores: 4,
                memory_total_bytes: None,
                memory_available_bytes: Some(128 * 1024 * 1024),
                ip_address: "10.0.0.1".into(),
            },
        ),
        (
            "larger".into(),
            WorkerSystemInfo {
                cpu_cores: 4,
                memory_total_bytes: None,
                memory_available_bytes: Some(896 * 1024 * 1024),
                ip_address: "10.0.0.2".into(),
            },
        ),
    ]);

    assert_eq!(
        worker_for_strategy(&workers, &details, RoutingStrategy::Balanced).as_deref(),
        Some("larger")
    );
    assert_eq!(
        worker_for_strategy(&workers, &details, RoutingStrategy::CpuFirst).as_deref(),
        Some("smaller"),
        "CPU First ignores RAM, so a CPU tie follows roster order"
    );
}

fn provider_budget(
    provider: HarnessProvider,
    limit_tokens: i64,
    remaining_tokens: i64,
) -> HarnessBudget {
    HarnessBudget {
        provider,
        seat: None,
        window: BudgetWindow::Weekly,
        limit_tokens: Some(limit_tokens),
        used_tokens: Some(limit_tokens - remaining_tokens),
        remaining_tokens: Some(remaining_tokens),
        cooldown_until: None,
        source: BudgetSource::Configured,
    }
}

#[test]
fn subscription_strategies_compare_percentage_and_absolute_budget_independently() {
    let capabilities = AgentCapabilities {
        providers: vec![HarnessProvider::Claude, HarnessProvider::Codex],
        budgets: vec![
            provider_budget(HarnessProvider::Claude, 1_000, 800),
            provider_budget(HarnessProvider::Codex, 10_000, 2_000),
        ],
        ..Default::default()
    };

    assert_eq!(
        subscription_for_strategy(&capabilities, SubscriptionRoutingStrategy::Balanced),
        Some(HarnessProvider::Claude),
        "balanced compares normalized headroom"
    );
    assert_eq!(
        subscription_for_strategy(
            &capabilities,
            SubscriptionRoutingStrategy::MostAvailableBudget
        ),
        Some(HarnessProvider::Codex),
        "most-available compares absolute remaining tokens"
    );
    assert_eq!(
        subscription_for_strategy(&capabilities, SubscriptionRoutingStrategy::Manual),
        None,
        "manual preserves the task hint or daemon default"
    );
}

#[test]
fn subscription_routing_excludes_not_ready_and_fails_open_without_numbers() {
    let capabilities = AgentCapabilities {
        providers: vec![HarnessProvider::Claude, HarnessProvider::Codex],
        budgets: vec![
            provider_budget(HarnessProvider::Claude, 1_000, 900),
            provider_budget(HarnessProvider::Codex, 1_000, 100),
        ],
        readiness: vec![HarnessReadiness {
            provider: HarnessProvider::Claude,
            ready: false,
            reason: Some("cooldown".into()),
        }],
        ..Default::default()
    };

    assert_eq!(
        subscription_for_strategy(&capabilities, SubscriptionRoutingStrategy::Balanced),
        Some(HarnessProvider::Codex)
    );
    assert_eq!(
        subscription_for_strategy(
            &AgentCapabilities {
                providers: vec![HarnessProvider::Claude],
                ..Default::default()
            },
            SubscriptionRoutingStrategy::MostAvailableBudget
        ),
        None,
        "missing advisory budget data falls back to the daemon default"
    );
}

#[test]
fn register_payload_advertises_id_address_and_harness() {
    let payload = register_payload(&[worker("w1", "GRVaddr")], &no_presence(), &[], &[]);
    let agents = payload.get("agents").unwrap().as_array().unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0]["id"], "w1");
    assert_eq!(agents[0]["metadata"]["address"], "GRVaddr");
    assert_eq!(agents[0]["metadata"]["harness"], "claude");
}

/// A worker whose workspace this hub knows must advertise it, because that is
/// what the backend turns into a `WorkspaceDescriptor` and places the agent in.
/// Without it the orchestrator reads the fleet as "no workspaces declared" and
/// declines work it could have delegated.
#[test]
fn register_payload_advertises_a_known_workspace() {
    let mut w = worker("this-device", "this-device");
    w.workspace = Some(crate::runtime::WorkspaceRef::checkout("/srv/repos/medulla"));
    let payload = register_payload(&[w], &no_presence(), &[], &[]);
    let agents = payload.get("agents").unwrap().as_array().unwrap();
    assert_eq!(agents[0]["metadata"]["workspace"], "/srv/repos/medulla");
}

/// An unknown workspace omits the key rather than sending an empty string: the
/// backend falls back to the worker's probed `capabilities.cwd`, and `""` would
/// win that fallback and place the agent nowhere.
#[test]
fn register_payload_omits_an_unknown_or_blank_workspace() {
    let payload = register_payload(&[worker("w1", "GRVaddr")], &no_presence(), &[], &[]);
    let agents = payload.get("agents").unwrap().as_array().unwrap();
    assert!(agents[0]["metadata"].get("workspace").is_none());

    let mut blank = worker("w2", "ADDR2");
    blank.workspace = Some(crate::runtime::WorkspaceRef::checkout("   "));
    let payload = register_payload(&[blank], &no_presence(), &[], &[]);
    let agents = payload.get("agents").unwrap().as_array().unwrap();
    assert!(agents[0]["metadata"].get("workspace").is_none());
}

#[test]
fn an_absent_agent_id_falls_back_but_an_unknown_one_does_not() {
    // These were one case and are two. An absent id means "any worker" — the
    // backend omits it for an unattributed task. An id that is present but
    // unrecognised means something addressed a specific agent this hub does not
    // have, and running that on whichever worker sorted first is a wrong answer
    // dressed as a right one.
    let workers = [worker("w1", "ADDR1"), worker("w2", "ADDR2")];
    assert_eq!(address_of(&workers, "w2").as_deref(), Some("ADDR2"));
    assert_eq!(address_of(&workers, "").as_deref(), Some("ADDR1"));
    assert_eq!(address_of(&workers, "   ").as_deref(), Some("ADDR1"));
    assert_eq!(
        address_of(&workers, "unknown"),
        None,
        "an unrecognised target must be refused, not guessed at"
    );
    assert_eq!(address_of(&[], "w1"), None);
}

#[test]
fn a_worker_is_addressable_by_its_cryptoid_too() {
    // A roster saved before ids were human-scale stored the cryptoId *as* the
    // id, and `MEDULLA_HUB_WORKERS` can still pin one. Both must keep resolving
    // or an upgrade silently unaddresses every existing worker.
    let workers = [worker("claude-worker", "3Hob1FxUwsy")];
    assert_eq!(
        address_of(&workers, "3Hob1FxUwsy").as_deref(),
        Some("3Hob1FxUwsy")
    );
    assert_eq!(
        address_of(&workers, "claude-worker").as_deref(),
        Some("3Hob1FxUwsy")
    );
}

#[test]
fn an_advertised_worker_is_online_so_it_can_be_auto_assigned() {
    // The orchestrator auto-assigns an untargeted task only to an agent whose
    // availability is exactly "online". Advertising a blank one excluded this
    // hub's workers from every fan-out, and rendered as an empty column in
    // agent_list — which reads as a broken row, not an idle worker.
    let payload = register_payload(&[worker("w1", "GRVaddr")], &no_presence(), &[], &[]);
    let agents = payload.get("agents").unwrap().as_array().unwrap();
    assert_eq!(agents[0]["availability"], "online");
}

#[test]
fn a_worker_id_is_short_stable_and_unique() {
    use super::super::roster::worker_id;
    // The id is what the orchestrator must reproduce to address the worker; a
    // 44-character base58 cryptoId reads as noise beside a memorable name, and
    // the model reaches for the name.
    assert_eq!(worker_id(None, "claude", &[]), "claude-worker");
    assert_eq!(
        worker_id(Some("Sanil Laptop"), "claude", &[]),
        "sanil-laptop"
    );
    assert_eq!(worker_id(Some("  "), "codex", &[]), "codex-worker");
    // Distinct even when two unlabelled workers share a harness — otherwise one
    // shadows the other in the backend registry.
    let taken = vec!["claude-worker".to_string()];
    assert_eq!(worker_id(None, "claude", &taken), "claude-worker-2");
    // Nothing usable in the label falls back rather than producing an empty id.
    assert_eq!(worker_id(Some("!!!"), "claude", &[]), "claude-worker");
}

#[test]
fn address_of_prefers_the_selected_worker_over_the_first() {
    let mut selected = worker("w2", "ADDR2");
    selected.selected = true;
    let workers = [worker("w1", "ADDR1"), selected];
    // An explicit match still wins.
    assert_eq!(address_of(&workers, "w1").as_deref(), Some("ADDR1"));
    // An ABSENT agentId routes to the SELECTED worker, which is what makes
    // `select()` a real dispatch control rather than a display flag.
    assert_eq!(address_of(&workers, "").as_deref(), Some("ADDR2"));
    // An unrecognised one is refused even with a selection: "any worker" and
    // "that worker, which I do not have" are different requests.
    assert_eq!(address_of(&workers, "unknown"), None);
}

#[test]
fn adding_a_peer_requests_contact_unless_it_is_already_one() {
    use super::super::handle::should_request_contact;

    assert!(
        should_request_contact("peer-address", false),
        "a new peer must be asked"
    );
    assert!(
        should_request_contact("peer-address", false),
        "and a duplicate re-asked, which is how a missed request is retried"
    );
    assert!(
        !should_request_contact("peer-address", true),
        "an accepted contact has nothing left to ask for"
    );
    assert!(
        !should_request_contact("", false),
        "a worker with no address has nobody to ask"
    );
}

// ------------------------------------------------------------- roster dedupe ---

fn hw(id: &str, address: &str) -> HubWorker {
    HubWorker {
        roles: Vec::new(),
        id: id.to_string(),
        address: address.to_string(),
        harness: "claude".to_string(),
        label: None,
        selected: false,
        workspace: None,
        ..Default::default()
    }
}

#[test]
fn one_peer_never_occupies_two_roster_slots() {
    use super::super::roster::remove_conflicting;

    // `MEDULLA_HUB_WORKERS="alpha=<addr>"` seeds the id `alpha`; adding the same
    // address in the TUI uses the address as the id. Same wallet, two names.
    let mut roster = vec![hw("alpha", "So1anaAddr")];
    let incoming = hw("So1anaAddr", "So1anaAddr");
    let removed = remove_conflicting(&mut roster, &incoming);
    roster.push(incoming);

    assert_eq!(removed, vec!["alpha"]);
    assert_eq!(roster.len(), 1, "one destination, one entry");
    assert_eq!(roster[0].id, "So1anaAddr", "the newest naming wins");
}

#[test]
fn re_adding_the_same_id_still_replaces() {
    use super::super::roster::remove_conflicting;

    let mut roster = vec![hw("w1", "addr-a")];
    let incoming = hw("w1", "addr-b");
    let removed = remove_conflicting(&mut roster, &incoming);
    roster.push(incoming);

    assert_eq!(removed, vec!["w1"]);
    assert_eq!(roster.len(), 1);
    assert_eq!(roster[0].address, "addr-b", "an id can be repointed");
}

#[test]
fn distinct_peers_are_left_alone() {
    use super::super::roster::remove_conflicting;

    let mut roster = vec![hw("w1", "addr-a"), hw("w2", "addr-b")];
    let incoming = hw("w3", "addr-c");
    let removed = remove_conflicting(&mut roster, &incoming);
    roster.push(incoming);

    assert!(removed.is_empty());
    assert_eq!(roster.len(), 3, "deduping must not collapse real peers");
}

#[test]
fn blank_addresses_do_not_collide_with_each_other() {
    // Two entries with no address are not "the same peer"; collapsing them would
    // silently delete a roster row on an unrelated add.
    use super::super::roster::remove_conflicting;

    let mut roster = vec![hw("w1", "")];
    let incoming = hw("w2", "");
    remove_conflicting(&mut roster, &incoming);
    roster.push(incoming);

    assert_eq!(roster.len(), 2);
}

#[test]
fn a_handle_is_recognised_as_an_alias_not_an_address() {
    use super::super::handle::is_handle;

    // Contacts, pre-key bundles and DMs are all keyed on the cryptoId; an
    // `@handle` is only a directory alias, and passing it through unresolved
    // produces `POST /contacts/%40name`, which cannot match anything.
    assert!(is_handle("@alice"));
    assert!(is_handle("  @alice"), "leading space is still a handle");
    assert!(
        !is_handle("8m6ZTfUGMdnoWanb1V31SZncBfr9xA1oAXnkv4cAAHVB"),
        "a cryptoId is already the key"
    );
    assert!(!is_handle(""));
}

#[test]
fn an_implausible_address_is_refused_before_it_reaches_the_relay() {
    use super::super::handle::is_plausible_address;

    // A stray `>` was accepted as a worker address, registered in the roster,
    // and had a contact request sent to it. Nothing downstream can tell that
    // from a real peer that simply never replies.
    assert!(!is_plausible_address(">"));
    assert!(!is_plausible_address(""));
    assert!(!is_plausible_address("   "));
    assert!(!is_plausible_address("too-short"));
    assert!(
        !is_plausible_address("3Hob1FxUwsy1K2rweppbmCkuPef6unAr5Amj6kQ2fM0A"),
        "base58 excludes 0, O, I and l because they are easy to confuse"
    );

    // Real values must still pass.
    assert!(is_plausible_address(
        "3Hob1FxUwsy1K2rweppbmCkuPef6unAr5Amj6kQ2fM3A"
    ));
    assert!(is_plausible_address(
        "8m6ZTfUGMdnoWanb1V31SZncBfr9xA1oAXnkv4cAAHVB"
    ));
    assert!(is_plausible_address("@alice"));
    assert!(!is_plausible_address("@"), "a bare @ names nobody");
}

#[test]
fn an_unlabelled_worker_advertises_one_token_not_two() {
    // `agent_list` renders `id (name)`. When those differ and both read as
    // names, the model picks one and may pick the unroutable one — which is the
    // original bug. Unlabelled, they must coincide.
    let payload = register_payload(
        &[worker("claude-worker", "3Hob1Fxu")],
        &no_presence(),
        &[],
        &[],
    );
    let agents = payload.get("agents").unwrap().as_array().unwrap();
    assert_eq!(agents[0]["id"], "claude-worker");
    assert_eq!(
        agents[0]["name"], "claude-worker",
        "an unlabelled worker must not advertise a second, different name"
    );

    // A labelled one keeps its human name; the id stays a visible slug of it.
    let mut labelled = worker("sanil-laptop", "3Hob1Fxu");
    labelled.label = Some("Sanil Laptop".to_string());
    let payload = register_payload(&[labelled], &no_presence(), &[], &[]);
    let agents = payload.get("agents").unwrap().as_array().unwrap();
    assert_eq!(agents[0]["id"], "sanil-laptop");
    assert_eq!(agents[0]["name"], "Sanil Laptop");
}

#[test]
fn a_worker_the_relay_reports_down_is_withheld_entirely() {
    // Not advertised as offline — withheld. Marking it down only stops the
    // *automatic* assignment of an untargeted task; a task naming it still
    // resolves and dispatches into the void, which is the stall being fixed.
    let online = std::collections::HashMap::from([("GRVdead".to_string(), false)]);
    let payload = register_payload(
        &[worker("live", "GRVlive"), worker("dead", "GRVdead")],
        &online,
        &[],
        &[],
    );
    let agents = payload["agents"].as_array().expect("an agent list");

    assert_eq!(agents.len(), 1, "only the reachable one is offered");
    assert_eq!(agents[0]["id"], "live");
    // What survives is advertised online, because the orchestrator only
    // auto-assigns to an agent whose availability is exactly that.
    assert_eq!(agents[0]["availability"], "online");
}

#[test]
fn a_worker_the_relay_reports_up_is_advertised() {
    let online = std::collections::HashMap::from([("GRVaddr".to_string(), true)]);
    let payload = register_payload(&[worker("w1", "GRVaddr")], &online, &[], &[]);
    assert_eq!(payload["agents"].as_array().expect("agents").len(), 1);
    assert_eq!(payload["agents"][0]["availability"], "online");
}

#[test]
fn no_answer_from_the_relay_advertises_everything() {
    // "The relay did not say" is not "the worker is down". One dropped request
    // must not empty a live roster.
    let payload = register_payload(
        &[worker("w1", "GRVone"), worker("w2", "GRVtwo")],
        &no_presence(),
        &[],
        &[],
    );
    assert_eq!(payload["agents"].as_array().expect("agents").len(), 2);
}

#[test]
fn every_roster_address_is_asked_about_in_one_batch() {
    // One request for the whole roster, not one per worker: a hub with a dozen
    // workers should not spend a dozen round trips to redraw one column.
    let workers = [worker("w1", "GRVone"), worker("w2", "GRVtwo")];
    assert_eq!(addresses_of(&workers), vec!["GRVone", "GRVtwo"]);
}

#[test]
fn the_withheld_addresses_are_reportable() {
    // Named in the log rather than silently dropped: a roster that quietly
    // shrinks is indistinguishable from one that was never configured.
    let online = std::collections::HashMap::from([
        ("GRVdead".to_string(), false),
        ("GRVlive".to_string(), true),
    ]);
    let workers = [worker("live", "GRVlive"), worker("dead", "GRVdead")];
    assert_eq!(unreachable_addresses(&workers, &online), vec!["GRVdead"]);
}

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

    let w = super::super::roster::worker_from_spec(&spec);
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
    // The declared host and capacity now reach the wire. `hostId` sits on the
    // agent, not inside its metadata: that is where the backend reads it.
    assert_eq!(agent["hostId"], "this-device");
    assert!(agent["metadata"].get("hostId").is_none());
    assert_eq!(agent["metadata"]["maxSessions"], 1);
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

    let w = super::super::roster::worker_from_spec(&spec);
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
        super::super::roster::lane_id(&workers, "this-device-codex", Some("this-device")),
        "this-device-codex"
    );
    // An unattributed dispatch has no id to prefer, so the machine's first agent
    // is the honest answer.
    assert_eq!(
        super::super::roster::lane_id(&workers, "", Some("this-device")),
        "this-device"
    );
    // A worker addressed by its cryptoId still resolves to its id.
    let remote = [worker("alpha", "GRVaddr")];
    assert_eq!(
        super::super::roster::lane_id(&remote, "GRVaddr", Some("GRVaddr")),
        "alpha"
    );
    assert_eq!(super::super::roster::lane_id(&[], "nobody", None), "");
}

// ------------------------------------------------------- the topology block ---

/// One declared local host, as `local_hosts` resolves one.
fn declared(id: &str, name: &str) -> crate::config::LocalHostRef {
    crate::config::LocalHostRef {
        id: id.to_string(),
        name: name.to_string(),
        workspace: String::new(),
        primary: false,
    }
}

/// A worker placed on `host_id`, reached at that host's address.
fn placed(id: &str, host_id: &str) -> HubWorker {
    HubWorker {
        host_id: host_id.to_string(),
        address: host_id.to_string(),
        ..worker(id, host_id)
    }
}

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

/// The keys the backend's per-agent control ingestion and the manager ledger's
/// control folds read. A regression here is silent — the advert still parses,
/// the folds just stop seeing a hold — so the whole metadata object is pinned
/// rather than spot-checked.
#[test]
fn the_control_and_handoff_keys_keep_exactly_the_shape_the_backend_folds() {
    let mut held = placed("this-device", "this-device");
    held.workspace = Some(crate::runtime::WorkspaceRef::checkout("/repos/acme"));
    held.control = super::super::HandoffControl::Operator;
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
            "control": "operator",
            "controlReason": "pairing on the migration",
            "controlSince": 1_753_420_600_000i64,
        }),
        "control/controlReason/controlSince must not move, gain a wrapper, or change spelling"
    );

    // And the handoff brief, which only rides while the orchestrator holds it.
    let mut handed_back = worker("w1", "GRVaddr");
    handed_back.handoff = Some(super::super::HarnessHandoff {
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
    let handoff = &payload["agents"][0]["metadata"]["handoff"];
    assert_eq!(handoff["id"], "w_3-1");
    assert_eq!(handoff["sessionId"], "w_3");
    assert_eq!(handoff["workspacePath"], "/repos/acme");
}
