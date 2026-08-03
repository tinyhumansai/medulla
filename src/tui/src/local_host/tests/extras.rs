//! Additional hosts on one machine: addressing, naming, harness choice, and
//! failure isolation between them.

use medulla::bridge::LocalBridgeNetwork;
use medulla::config::HostSection;
use medulla::daemon::embedded::EmbeddedDaemonOptions;
use medulla::protocol::HarnessProvider;
use medulla_tui::worker::pty::PtyManager;

use crate::local_host::{
    all_host_addresses, display_name, extra_host_address, extra_options, host_address,
    options_from_config, start_all,
};

use super::env_with_only_claude;

#[test]
fn each_extra_host_gets_its_own_bus_address() {
    // Two hosts cannot share an address — the second `bind` fails — so an
    // operator who declares `[[hosts]]` without thinking about addressing would
    // otherwise get one working host and one startup error.
    let named = HostSection {
        name: "Backend API".to_string(),
        ..HostSection::default()
    };
    let anonymous = HostSection {
        address: String::new(),
        name: String::new(),
        ..HostSection::default()
    };
    let explicit = HostSection {
        address: "chosen-by-hand".to_string(),
        ..HostSection::default()
    };

    assert_eq!(extra_host_address(&named, 0), "local-backend-api");
    assert_eq!(extra_host_address(&anonymous, 3), "local-host-4");
    assert_eq!(extra_host_address(&explicit, 0), "chosen-by-hand");
}

#[test]
fn every_declared_address_is_known_without_starting_anything() {
    // Needed in exactly the case where none started: a roster saved while
    // hosting was on must not keep advertising local entries nothing answers.
    let primary = HostSection::default();
    let extras = [
        HostSection {
            name: "backend".to_string(),
            ..HostSection::default()
        },
        HostSection {
            address: "custom".to_string(),
            ..HostSection::default()
        },
    ];
    assert_eq!(
        all_host_addresses(&primary, &extras),
        vec![
            HostSection::default().address,
            "local-backend".to_string(),
            "custom".to_string()
        ]
    );
}

#[test]
fn an_unnamed_extra_is_named_for_the_directory_it_works_in() {
    // Several hosts on one machine differ only by where they work, so "this
    // device" repeated would describe none of them.
    let unnamed = HostSection {
        name: String::new(),
        ..HostSection::default()
    };
    assert_eq!(
        display_name(&unnamed, "/Users/me/Projects/backend", false),
        "backend"
    );
    // The primary keeps its name: it *is* the machine the operator is at.
    assert_eq!(
        display_name(&unnamed, "/Users/me/Projects/backend", true),
        "this device"
    );
    // An explicit name always wins.
    let named = HostSection {
        name: "API box".to_string(),
        ..HostSection::default()
    };
    assert_eq!(display_name(&named, "/anywhere", false), "API box");
}

#[tokio::test]
async fn a_failing_extra_does_not_take_the_other_hosts_down() {
    // One mistyped directory should cost that host, not hosting altogether.
    let env = env_with_only_claude();
    let network = LocalBridgeNetwork::new();
    let primary = HostSection::default();
    // Two extras that collide on one address: the second cannot bind.
    let extras = [
        HostSection {
            address: "duplicate".to_string(),
            ..HostSection::default()
        },
        HostSection {
            address: "duplicate".to_string(),
            ..HostSection::default()
        },
    ];
    let options = options_from_config(&primary, &env, None, None, None, true).expect("options");

    let (hosts, problems) = start_all(
        &primary,
        &extras,
        &env,
        &network,
        options,
        PtyManager::new(),
    );

    assert_eq!(hosts.len(), 2, "the primary and the first extra both start");
    assert_eq!(
        problems.len(),
        1,
        "and the collision is reported, not fatal"
    );
}

#[test]
fn an_extra_runs_the_harness_it_declared_rather_than_the_primarys() {
    // The Add Host wizard asks which harness a local host runs and writes the
    // answer to `providers`/`defaultProvider`. Inheriting the primary's meant
    // the answer was persisted and then never read — a host added as codex ran
    // claude because the primary did.
    let primary = EmbeddedDaemonOptions {
        providers: Some(vec![HarnessProvider::Claude]),
        default_provider: Some(HarnessProvider::Claude),
        workspace: "/primary".to_string(),
        ..Default::default()
    };
    let extra = HostSection {
        workspace: "/extra".to_string(),
        providers: vec!["codex".to_string()],
        default_provider: "codex".to_string(),
        ..HostSection::default()
    };

    let options = extra_options(&primary, &extra).expect("valid providers");
    assert_eq!(options.default_provider, Some(HarnessProvider::Codex));
    assert_eq!(options.providers, Some(vec![HarnessProvider::Codex]));
    assert_eq!(options.workspace, "/extra");
}

#[test]
fn an_extra_that_names_no_harness_still_inherits_the_primarys() {
    // A `[[hosts]]` entry that is only a directory keeps behaving as one.
    let primary = EmbeddedDaemonOptions {
        providers: Some(vec![HarnessProvider::Claude]),
        default_provider: Some(HarnessProvider::Claude),
        ..Default::default()
    };
    let extra = HostSection {
        workspace: "/extra".to_string(),
        ..HostSection::default()
    };

    let options = extra_options(&primary, &extra).expect("no providers is fine");
    assert_eq!(options.default_provider, Some(HarnessProvider::Claude));
    assert_eq!(options.providers, Some(vec![HarnessProvider::Claude]));
}

#[test]
fn an_extras_unknown_harness_is_rejected_rather_than_silently_widening() {
    // Same rule the primary follows: an empty provider list means "detect
    // everything installed", so dropping a typo would widen what this machine
    // runs when the operator meant to narrow it.
    let extra = HostSection {
        workspace: "/extra".to_string(),
        providers: vec!["claudde".to_string()],
        ..HostSection::default()
    };
    assert!(extra_options(&EmbeddedDaemonOptions::default(), &extra).is_err());
}

#[test]
fn an_unnamed_extras_address_is_derived_from_its_config_index() {
    // `spawn` used to count *started* hosts, which includes the primary, so a
    // first unnamed extra bound `local-host-2` this run and `local-host-1` on
    // the next launch — leaving the roster remembering an address nothing binds.
    // Every site now derives from the entry's position within `[[hosts]]`.
    let unnamed = HostSection {
        workspace: "/extra".to_string(),
        address: String::new(),
        ..HostSection::default()
    };
    let primary = HostSection::default();

    assert_eq!(extra_host_address(&unnamed, 0), "local-host-1");
    assert_eq!(
        all_host_addresses(&primary, std::slice::from_ref(&unnamed)),
        vec![host_address(&primary), "local-host-1".to_string()],
    );
}

#[test]
fn an_extra_that_replaces_the_provider_list_drops_a_default_outside_it() {
    // Provider-only is a valid configuration, and inheriting the primary's
    // default there names a harness this host was just told not to run — the
    // same wrong-harness outcome, reached without a typo.
    let primary = EmbeddedDaemonOptions {
        providers: Some(vec![HarnessProvider::Claude]),
        default_provider: Some(HarnessProvider::Claude),
        ..Default::default()
    };
    let extra = HostSection {
        workspace: "/extra".to_string(),
        providers: vec!["codex".to_string()],
        default_provider: String::new(),
        ..HostSection::default()
    };

    let options = extra_options(&primary, &extra).expect("valid providers");
    assert_eq!(options.providers, Some(vec![HarnessProvider::Codex]));
    assert_eq!(
        options.default_provider, None,
        "the inherited default is not in the new list, so it is not a default"
    );
}

#[test]
fn an_inherited_default_survives_when_the_new_list_still_allows_it() {
    // Widening rather than replacing: claude is still permitted, so the
    // primary's default is still a sensible answer and clearing it would make
    // the host pick arbitrarily.
    let primary = EmbeddedDaemonOptions {
        providers: Some(vec![HarnessProvider::Claude]),
        default_provider: Some(HarnessProvider::Claude),
        ..Default::default()
    };
    let extra = HostSection {
        workspace: "/extra".to_string(),
        providers: vec!["claude".to_string(), "codex".to_string()],
        default_provider: String::new(),
        ..HostSection::default()
    };

    let options = extra_options(&primary, &extra).expect("valid providers");
    assert_eq!(options.default_provider, Some(HarnessProvider::Claude));
}
