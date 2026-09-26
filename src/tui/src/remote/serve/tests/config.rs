//! That a host launches agents with *its own* configuration, and advertises
//! what it can start.
//!
//! The part the Docker suite cannot see: a coding agent started from across the
//! network must be launched by the host's `[router]`, `[[hooks]]`,
//! `[attribution]` and presets, never the client's.

use super::support::*;
/// A preset as an operator would have written it on the host's own machine.
///
/// Built through the editor-line parser rather than a struct literal, because
/// that is the only constructor `CustomHarnessConfig` offers — and using it
/// keeps this test honest about what a real config produces.
fn host_preset() -> medulla::config::CustomHarnessConfig {
    medulla::config::CustomHarnessConfig::from_editor_line(
        "host-only | Host Only | claude | some-model |  | this-device",
    )
    .expect("a well-formed preset line")
}

#[test]
fn a_host_launches_agents_with_its_own_configuration() {
    // The point of stage 7, and the part the Docker suite cannot see: a coding
    // agent started from across the network must be launched by the *host's*
    // config, not the client's. Its `[router]` names that machine's keys, its
    // `[[hooks]]` are that machine's commands, and its presets describe binaries
    // that exist over there. Shipping the client's would be both wrong and a way
    // to make one machine run another's commands.
    let mut config = medulla::config::TuiConfig::default();
    config.attribution.commit = false;
    config.router = Some(medulla::config::RouterConfig::default());
    let preset = host_preset();

    let sessions = crate::remote::serve::entry::local_sessions(
        &HashMap::new(),
        &config,
        std::slice::from_ref(&preset),
        "/work",
    );

    assert!(
        !sessions.attribution,
        "the host's attribution setting must be the one that applies"
    );
    assert!(
        sessions.router.is_some(),
        "the host's router must reach the launch"
    );
    assert_eq!(
        sessions.custom_harnesses.len(),
        1,
        "a preset defined on the host is offerable to a client that has never heard of it"
    );
    assert_eq!(sessions.custom_harnesses[0].id, "host-only");
    assert_eq!(sessions.workspace, "/work");
}

#[test]
fn a_hosts_own_preset_is_advertised_to_clients() {
    // Follows from the above, and is what the picker actually reads: a preset
    // the host defines has to appear in the capabilities it sends back, or a
    // client could never select it.
    let mut config = medulla::config::TuiConfig::default();
    let preset = host_preset();
    config.attribution.commit = true;
    let sessions = crate::remote::serve::entry::local_sessions(
        &HashMap::new(),
        &config,
        std::slice::from_ref(&preset),
        "/work",
    );
    let network = LocalBridgeNetwork::new();
    let server = RemoteServer::new(
        sessions,
        Arc::new(network.bind("daemon-preset").expect("binds")),
        "host".to_string(),
    );
    assert!(
        server
            .capabilities()
            .harnesses
            .iter()
            .any(|choice| choice.id == "host-only"),
        "the host's own preset should be on offer: {:?}",
        server.capabilities().harnesses
    );
}
