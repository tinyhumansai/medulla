//! Tests for remote-host resolution: ids, names, and what gets dropped.

use super::{remote_host_id, remote_host_name, remote_hosts, RemoteHostSection};

/// A minimal usable entry — a host and nothing else, which is the shape an
/// operator writes first.
fn entry(host: &str) -> RemoteHostSection {
    RemoteHostSection {
        host: host.to_string(),
        enabled: true,
        ..RemoteHostSection::default()
    }
}

#[test]
fn an_entry_with_only_a_host_still_gets_an_id_and_a_name() {
    let hosts = remote_hosts(&[entry("tower.local")]);
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].id, "tower-local");
    assert_eq!(hosts[0].name, "tower.local");
    assert_eq!(hosts[0].host, "tower.local");
}

#[test]
fn an_explicit_id_outranks_the_name_and_the_host() {
    let section = RemoteHostSection {
        id: "gpu".to_string(),
        name: "GPU Box".to_string(),
        ..entry("gpu.example.com")
    };
    assert_eq!(remote_host_id(&section, 0), "gpu");
}

#[test]
fn the_name_is_used_before_the_host() {
    // The name is stable across a change of address, which the host is not — a
    // machine that moves networks should not orphan its sessions.
    let section = RemoteHostSection {
        name: "GPU Box".to_string(),
        ..entry("gpu.example.com")
    };
    assert_eq!(remote_host_id(&section, 0), "gpu-box");
    assert_eq!(remote_host_name(&section), "GPU Box");
}

#[test]
fn an_entry_with_no_host_is_dropped() {
    // There is nothing to dial, so a row for it could only fail when clicked.
    let hosts = remote_hosts(&[RemoteHostSection {
        name: "half-written".to_string(),
        enabled: true,
        ..RemoteHostSection::default()
    }]);
    assert!(hosts.is_empty());
}

#[test]
fn a_disabled_entry_is_not_offered_but_is_not_a_collision() {
    let hosts = remote_hosts(&[
        RemoteHostSection {
            enabled: false,
            ..entry("away.local")
        },
        entry("here.local"),
    ]);
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].host, "here.local");
}

#[test]
fn two_entries_that_slug_to_one_id_keep_the_first() {
    // Ids name sessions, so two hosts sharing one would make a remote session
    // ambiguous. Same rule, and same reason, as local hosts.
    let hosts = remote_hosts(&[
        RemoteHostSection {
            name: "GPU Box".to_string(),
            ..entry("first.local")
        },
        RemoteHostSection {
            name: "gpu box".to_string(),
            ..entry("second.local")
        },
    ]);
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].host, "first.local");
}

#[test]
fn a_positional_id_is_the_last_resort() {
    let section = RemoteHostSection {
        host: "!!!".to_string(),
        enabled: true,
        ..RemoteHostSection::default()
    };
    assert_eq!(remote_host_id(&section, 2), "remote-host-3");
}

#[test]
fn declaration_order_is_preserved() {
    let hosts = remote_hosts(&[entry("a.local"), entry("b.local"), entry("c.local")]);
    let ids: Vec<&str> = hosts.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(ids, ["a-local", "b-local", "c-local"]);
}
