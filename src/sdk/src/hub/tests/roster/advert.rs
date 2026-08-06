//! The advertised payload and how an `agentId` resolves back to an address.

use super::super::super::roster::{address_of, register_payload};
use super::helpers::{no_presence, worker};

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
    use super::super::super::roster::worker_id;
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
