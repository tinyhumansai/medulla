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
use crate::runtime::{RoutingStrategy, SubscriptionRoutingStrategy};
use crate::tinyplace::{
    AgentCapabilities, BudgetSource, BudgetWindow, HarnessBudget, HarnessProvider,
    HarnessReadiness, WorkerSystemInfo,
};

fn worker(id: &str, addr: &str) -> HubWorker {
    HubWorker {
        roles: Vec::new(),
        id: id.to_string(),
        address: addr.to_string(),
        harness: "claude".to_string(),
        label: None,
        selected: false,
        workspace: None,
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
    let payload = register_payload(&[worker("w1", "GRVaddr")], &no_presence(), &[]);
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
    w.workspace = Some("/srv/repos/medulla".to_string());
    let payload = register_payload(&[w], &no_presence(), &[]);
    let agents = payload.get("agents").unwrap().as_array().unwrap();
    assert_eq!(agents[0]["metadata"]["workspace"], "/srv/repos/medulla");
}

/// An unknown workspace omits the key rather than sending an empty string: the
/// backend falls back to the worker's probed `capabilities.cwd`, and `""` would
/// win that fallback and place the agent nowhere.
#[test]
fn register_payload_omits_an_unknown_or_blank_workspace() {
    let payload = register_payload(&[worker("w1", "GRVaddr")], &no_presence(), &[]);
    let agents = payload.get("agents").unwrap().as_array().unwrap();
    assert!(agents[0]["metadata"].get("workspace").is_none());

    let mut blank = worker("w2", "ADDR2");
    blank.workspace = Some("   ".to_string());
    let payload = register_payload(&[blank], &no_presence(), &[]);
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
    let payload = register_payload(&[worker("w1", "GRVaddr")], &no_presence(), &[]);
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
    let payload = register_payload(&[worker("claude-worker", "3Hob1Fxu")], &no_presence(), &[]);
    let agents = payload.get("agents").unwrap().as_array().unwrap();
    assert_eq!(agents[0]["id"], "claude-worker");
    assert_eq!(
        agents[0]["name"], "claude-worker",
        "an unlabelled worker must not advertise a second, different name"
    );

    // A labelled one keeps its human name; the id stays a visible slug of it.
    let mut labelled = worker("sanil-laptop", "3Hob1Fxu");
    labelled.label = Some("Sanil Laptop".to_string());
    let payload = register_payload(&[labelled], &no_presence(), &[]);
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
    let payload = register_payload(&[worker("w1", "GRVaddr")], &online, &[]);
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

    let payload = register_payload(&[w], &no_presence(), &catalog);
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
    let payload = register_payload(&[w], &no_presence(), &crate::agents::default_templates());
    assert_eq!(payload["agents"][0]["description"], "claude daemon");
}
