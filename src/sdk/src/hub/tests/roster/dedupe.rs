//! One peer to one roster slot, and withholding the addresses the relay says
//! are unreachable — the two rules that keep an advert honest.

use super::super::super::roster::{addresses_of, register_payload, unreachable_addresses};
use super::helpers::{hw, no_presence, worker};

#[test]
fn one_peer_never_occupies_two_roster_slots() {
    use super::super::super::roster::remove_conflicting;

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
    use super::super::super::roster::remove_conflicting;

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
    use super::super::super::roster::remove_conflicting;

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
    use super::super::super::roster::remove_conflicting;

    let mut roster = vec![hw("w1", "")];
    let incoming = hw("w2", "");
    remove_conflicting(&mut roster, &incoming);
    roster.push(incoming);

    assert_eq!(roster.len(), 2);
}

#[test]
fn a_handle_is_recognised_as_an_alias_not_an_address() {
    use super::super::super::handle::is_handle;

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
    use super::super::super::handle::is_plausible_address;

    // A stray `>` was accepted as a worker address, registered in the roster,
    // and registered as a worker. Nothing downstream can tell that
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
