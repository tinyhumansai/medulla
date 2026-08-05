//! Starting hosts: what the hub is told, and what happens when two want the
//! same address.

use std::collections::HashMap;

use medulla::bridge::LocalBridgeNetwork;
use medulla::config::HostSection;
use medulla::protocol::HarnessProvider;
use medulla_tui::worker::pty::PtyManager;

use crate::local_host::{options_from_config, start};

use super::env_with_only_claude;

#[tokio::test]
async fn hosting_switched_off_is_a_choice_not_an_error() {
    let network = LocalBridgeNetwork::new();
    let config = HostSection {
        enabled: false,
        ..HostSection::default()
    };
    let options = options_from_config(
        &config,
        &env_with_only_claude(),
        None,
        None,
        None,
        &super::super::LaunchPolicy {
            attribution: true,
            ..Default::default()
        },
    )
    .expect("valid config");

    let host = start(
        &config,
        &HashMap::new(),
        &network,
        options,
        PtyManager::new(),
        &[],
    )
    .unwrap();

    assert!(host.is_none());
    // Nothing was bound, so the address is still free for a later run.
    assert!(network.bind("this-device").is_ok());
}

#[tokio::test]
async fn a_started_host_advertises_this_machine_to_the_hub() {
    let network = LocalBridgeNetwork::new();
    let config = HostSection::default();
    let env = env_with_only_claude();
    let options = options_from_config(
        &config,
        &env,
        None,
        None,
        None,
        &super::super::LaunchPolicy {
            attribution: true,
            ..Default::default()
        },
    )
    .expect("valid config");

    let host = start(
        &config,
        &HashMap::new(),
        &network,
        options,
        PtyManager::new(),
        &[],
    )
    .unwrap()
    .expect("hosting is on by default");

    assert_eq!(host.address(), "this-device");
    assert_eq!(host.providers(), [HarnessProvider::Claude]);
    // With nothing declared the daemon's own detection seeds one agent, and it
    // keeps the id, address and label the single pre-declaration entry had.
    assert_eq!(host.specs().len(), 1);
    let spec = &host.specs()[0];
    assert_eq!(spec.id, "this-device");
    assert_eq!(spec.host_id, "this-device");
    assert_eq!(spec.address, "this-device");
    assert_eq!(spec.name, "this device");
    assert_eq!(spec.harness, "claude");
    assert!(
        spec.description.contains("claude") && spec.description.contains(host.workspace()),
        "the roster entry should say what runs where: {}",
        spec.description
    );
    // Structured, not only prose: the backend places the agent from
    // `metadata.workspace`, and a path buried in the description does not
    // reach it — the orchestrator would see a host with no workspace at all.
    assert_eq!(
        spec.workspace.as_ref().map(|w| w.path.as_str()),
        Some(host.workspace())
    );
    assert_eq!(
        spec.max_sessions, 1,
        "a checkout runs one session at a time"
    );
    assert_eq!(host.observation().stats().tasks_started, 0);
    assert_eq!(host.observation().address(), "this-device");
}

#[tokio::test]
async fn a_second_host_on_one_address_is_refused_rather_than_splitting_the_inbox() {
    let network = LocalBridgeNetwork::new();
    let config = HostSection::default();
    let env = env_with_only_claude();

    let _first = start(
        &config,
        &HashMap::new(),
        &network,
        options_from_config(
            &config,
            &env,
            None,
            None,
            None,
            &super::super::LaunchPolicy {
                attribution: true,
                ..Default::default()
            },
        )
        .expect("valid config"),
        PtyManager::new(),
        &[],
    )
    .unwrap()
    .expect("the first host starts");

    let error = start(
        &config,
        &HashMap::new(),
        &network,
        options_from_config(
            &config,
            &env,
            None,
            None,
            None,
            &super::super::LaunchPolicy {
                attribution: true,
                ..Default::default()
            },
        )
        .expect("valid config"),
        PtyManager::new(),
        &[],
    )
    .unwrap_err();

    assert!(
        error.contains("could not host on this device"),
        "unexpected error: {error}"
    );
}
