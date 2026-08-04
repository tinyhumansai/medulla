//! Unit tests for device-local host resolution.

use super::{local_host_address, local_host_name, local_hosts, HostSection};

/// A `[[hosts]]` entry as `load` produces it: fields default to the primary's,
/// which is exactly why an unchosen address must not count as chosen.
fn extra(name: &str, workspace: &str) -> HostSection {
    HostSection {
        name: name.into(),
        workspace: workspace.into(),
        ..HostSection::default()
    }
}

#[test]
fn an_extra_address_comes_from_its_name_then_its_position() {
    let named = extra("backend API", "/w");
    let anonymous = extra("", "/w");
    let mut explicit = extra("ignored", "/w");
    explicit.address = "chosen-by-hand".into();

    assert_eq!(local_host_address(&named, 0), "local-backend-api");
    assert_eq!(local_host_address(&anonymous, 3), "local-host-4");
    assert_eq!(local_host_address(&explicit, 0), "chosen-by-hand");
}

#[test]
fn inheriting_the_primary_address_counts_as_unchosen() {
    // `[[hosts]]` shares `HostSection`, so an entry that names no address
    // deserializes with the primary's default. Treating that as a choice would
    // hand two hosts one address and the second would never bind.
    let mut inherited = extra("", "/w");
    inherited.address = HostSection::default().address;
    assert_eq!(local_host_address(&inherited, 0), "local-host-1");
}

#[test]
fn the_primary_leads_and_each_extra_follows_in_order() {
    let primary = HostSection {
        workspace: "/Users/me/medulla".into(),
        ..HostSection::default()
    };
    let extras = vec![
        extra("API", "/Users/me/Projects/backend"),
        extra("", "/tmp/x"),
    ];

    let hosts = local_hosts(&primary, &extras);
    let ids: Vec<&str> = hosts.iter().map(|host| host.id.as_str()).collect();
    assert_eq!(ids, vec!["this-device", "local-api", "local-host-2"]);
    assert!(hosts[0].primary);
    assert!(!hosts[1].primary);
    assert_eq!(hosts[0].name, "this device");
    assert_eq!(hosts[1].name, "API");
    // An unnamed extra is named for the directory that distinguishes it.
    assert_eq!(hosts[2].name, "x");
    assert_eq!(hosts[1].workspace, "/Users/me/Projects/backend");
}

#[test]
fn an_unnamed_host_falls_back_to_its_directory_then_the_path() {
    let unnamed = extra("", "");
    assert_eq!(
        local_host_name(&unnamed, "/Users/me/Projects/backend", false),
        "backend"
    );
    assert_eq!(local_host_name(&unnamed, "", false), "");
    assert_eq!(local_host_name(&unnamed, "/anything", true), "this device");
    assert_eq!(
        local_host_name(&extra("API box", ""), "/x", false),
        "API box"
    );
}
