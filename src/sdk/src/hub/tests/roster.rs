//! Tests for the hub roster: how a worker is advertised, addressed, and kept
//! unique.
//!
//! The roster is the only thing standing between an orchestrator's `agentId`
//! and a tiny.place address, so these pin the resolution rules rather than the
//! transport — dispatch itself is covered in [`super::super::dispatch`].

use super::super::roster::{address_of, register_payload, HubWorker};

fn worker(id: &str, addr: &str) -> HubWorker {
    HubWorker {
        id: id.to_string(),
        address: addr.to_string(),
        harness: "claude".to_string(),
        label: None,
        selected: false,
    }
}

#[test]
fn register_payload_advertises_id_address_and_harness() {
    let payload = register_payload(&[worker("w1", "GRVaddr")]);
    let agents = payload.get("agents").unwrap().as_array().unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0]["id"], "w1");
    assert_eq!(agents[0]["metadata"]["address"], "GRVaddr");
    assert_eq!(agents[0]["metadata"]["harness"], "claude");
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
    let payload = register_payload(&[worker("w1", "GRVaddr")]);
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
        id: id.to_string(),
        address: address.to_string(),
        harness: "claude".to_string(),
        label: None,
        selected: false,
    }
}

#[test]
fn one_peer_never_occupies_two_roster_slots() {
    use super::super::roster::remove_conflicting;

    // `MEDULLA_HUB_WORKERS="alpha=<addr>"` seeds the id `alpha`; adding the same
    // address in the TUI uses the address as the id. Same wallet, two names.
    let mut roster = vec![hw("alpha", "So1anaAddr")];
    let incoming = hw("So1anaAddr", "So1anaAddr");
    remove_conflicting(&mut roster, &incoming);
    roster.push(incoming);

    assert_eq!(roster.len(), 1, "one destination, one entry");
    assert_eq!(roster[0].id, "So1anaAddr", "the newest naming wins");
}

#[test]
fn re_adding_the_same_id_still_replaces() {
    use super::super::roster::remove_conflicting;

    let mut roster = vec![hw("w1", "addr-a")];
    let incoming = hw("w1", "addr-b");
    remove_conflicting(&mut roster, &incoming);
    roster.push(incoming);

    assert_eq!(roster.len(), 1);
    assert_eq!(roster[0].address, "addr-b", "an id can be repointed");
}

#[test]
fn distinct_peers_are_left_alone() {
    use super::super::roster::remove_conflicting;

    let mut roster = vec![hw("w1", "addr-a"), hw("w2", "addr-b")];
    let incoming = hw("w3", "addr-c");
    remove_conflicting(&mut roster, &incoming);
    roster.push(incoming);

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
    let payload = register_payload(&[worker("claude-worker", "3Hob1Fxu")]);
    let agents = payload.get("agents").unwrap().as_array().unwrap();
    assert_eq!(agents[0]["id"], "claude-worker");
    assert_eq!(
        agents[0]["name"], "claude-worker",
        "an unlabelled worker must not advertise a second, different name"
    );

    // A labelled one keeps its human name; the id stays a visible slug of it.
    let mut labelled = worker("sanil-laptop", "3Hob1Fxu");
    labelled.label = Some("Sanil Laptop".to_string());
    let payload = register_payload(&[labelled]);
    let agents = payload.get("agents").unwrap().as_array().unwrap();
    assert_eq!(agents[0]["id"], "sanil-laptop");
    assert_eq!(agents[0]["name"], "Sanil Laptop");
}
