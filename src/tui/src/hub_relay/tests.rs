//! Tests for the `MEDULLA_HUB` enable gate.

use std::collections::HashMap;

use super::hub_enabled;

/// Build an environment map from `(key, value)` pairs.
fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn on_by_default_in_backend_mode() {
    // A plain login (no hub-related vars) still runs the hub.
    assert!(hub_enabled(&env(&[])));
    // A pre-seeded worker also runs it (unchanged).
    assert!(hub_enabled(&env(&[(
        "MEDULLA_TINYPLACE_PEER",
        "GRV1worker"
    )])));
}

#[test]
fn explicit_zero_is_a_hard_kill_switch() {
    assert!(!hub_enabled(&env(&[("MEDULLA_HUB", "0")])));
    assert!(!hub_enabled(&env(&[("MEDULLA_HUB", "false")])));
    // The kill-switch wins even when a worker is configured.
    assert!(!hub_enabled(&env(&[
        ("MEDULLA_HUB", "0"),
        ("MEDULLA_TINYPLACE_PEER", "GRV1worker"),
    ])));
}

#[test]
fn explicit_truthy_is_on() {
    assert!(hub_enabled(&env(&[("MEDULLA_HUB", "1")])));
    assert!(hub_enabled(&env(&[("MEDULLA_HUB", "true")])));
    // A blank value is ignored → falls back to the default (on).
    assert!(hub_enabled(&env(&[("MEDULLA_HUB", "  ")])));
}

#[test]
fn the_hub_never_writes_to_the_terminal_the_tui_owns() {
    // Regression. The hub used to `eprintln!` its progress — "hub: connecting to
    // <url>", "socket closed — reconnecting", every task result. Under the
    // orchestrator TUI that lands on top of the alternate screen, and ratatui
    // only repaints the cells it manages, so the text never clears.
    //
    // Asserted against the source rather than at runtime: the failure is a
    // stray write from a background task, which no unit test would observe.
    for path in [
        "src/sdk/src/hub/boot.rs",
        "src/sdk/src/hub/socket.rs",
        "src/tui/src/hub_relay.rs",
    ] {
        let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(path);
        let Ok(source) = std::fs::read_to_string(&full) else {
            continue; // not laid out as expected; nothing to assert
        };
        let offenders: Vec<&str> = source
            .lines()
            .filter(|line| line.contains("eprintln!") || line.contains("println!"))
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect();
        assert!(
            offenders.is_empty(),
            "{path} writes to the terminal; route it through the hub log sink instead: {offenders:?}"
        );
    }
}

// --------------------------------------------------------------- roster ---

/// A live roster entry, as the hub holds it.
fn worker(id: &str, address: &str, selected: bool) -> medulla::hub::HubWorker {
    medulla::hub::HubWorker {
        roles: Vec::new(),
        id: id.to_string(),
        address: address.to_string(),
        harness: "claude".to_string(),
        label: Some("laptop".to_string()),
        selected,
        workspace: None,
    }
}

#[test]
fn a_saved_roster_comes_back_on_the_next_launch() {
    // The bug this exists to close: the roster lived only in memory, seeded from
    // the environment at boot, so a worker added in the Workers tab was gone at
    // exit and the tab was empty next time however many peers were reachable.
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path();

    assert!(
        super::workers_from_config(home).is_empty(),
        "nothing remembered before anything is saved"
    );

    let sink = super::roster_sink(home, medulla::hub::stderr_log(), shared(Vec::new()));
    sink(&[
        worker("alpha", "3Hob1Fxu", true),
        worker("beta", "@peer", false),
    ]);

    let specs = super::workers_from_config(home);
    assert_eq!(specs.len(), 2, "got {specs:?}");
    assert_eq!(specs[0].id, "alpha");
    assert_eq!(specs[0].address, "3Hob1Fxu");
    assert_eq!(specs[0].harness, "claude");
    assert_eq!(specs[1].address, "@peer");
}

#[test]
fn an_explicit_environment_roster_is_not_merged_with_the_saved_one() {
    // An exported roster is a deliberate override for this run. Merging would
    // quietly re-add a worker the operator had removed.
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path();
    super::roster_sink(home, medulla::hub::stderr_log(), shared(Vec::new()))(&[worker(
        "saved",
        "addr-saved",
        false,
    )]);

    let from_env = super::workers_from_env(&env(&[("MEDULLA_TINYPLACE_PEER", "addr-env")]));
    assert_eq!(from_env.len(), 1);
    assert_eq!(from_env[0].address, "addr-env");
    // And the saved one is still on disk, untouched, for a run without the var.
    assert_eq!(super::workers_from_config(home)[0].address, "addr-saved");
}

#[test]
fn saving_over_a_config_leaves_its_other_sections_alone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path();
    std::fs::write(
        home.join("config.toml"),
        "[onboarding]\nwelcomeCompleted = true\n",
    )
    .expect("seed");

    super::roster_sink(home, medulla::hub::stderr_log(), shared(Vec::new()))(&[worker(
        "alpha", "addr", false,
    )]);

    let text = std::fs::read_to_string(home.join("config.toml")).expect("read");
    assert!(text.contains("welcomeCompleted"), "got: {text}");
    assert!(text.contains("addr"), "got: {text}");
}

/// The shared device-local address list the sink reads at save time.
fn shared(addresses: Vec<String>) -> std::sync::Arc<std::sync::Mutex<Vec<String>>> {
    std::sync::Arc::new(std::sync::Mutex::new(addresses))
}

#[test]
fn an_unwritable_roster_path_does_not_take_the_hub_down() {
    // Losing the roster is a nuisance; failing to start is an outage.
    let sink = super::roster_sink(
        std::path::Path::new("/proc/nonexistent/nope"),
        medulla::hub::stderr_log(),
        shared(Vec::new()),
    );
    sink(&[worker("alpha", "addr", false)]);
}

#[test]
fn the_device_local_host_is_never_written_into_the_saved_roster() {
    // It is derived from `[host]` on every launch, so remembering it would
    // outlive the setting that produced it.
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path();

    super::roster_sink(
        home,
        medulla::hub::stderr_log(),
        shared(vec!["this-device".to_string()]),
    )(&[
        worker("this-device", "this-device", false),
        worker("beta", "3Hob1Fxu", false),
    ]);

    let saved = super::workers_from_config(home);
    assert_eq!(saved.len(), 1, "only the remote worker persists: {saved:?}");
    assert_eq!(saved[0].address, "3Hob1Fxu");
}

#[test]
fn a_roster_remembered_from_a_hosting_run_is_dropped_when_hosting_is_off() {
    // The regression: a roster saved by an older build (or any run that wrote
    // the entry) would keep advertising `this-device` after `MEDULLA_HOST=0`.
    // With no local endpoint bound, the router finds no match and sends its
    // tasks over tiny.place to a name no relay can resolve.
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path();
    // Write it the way a build without the filter would have.
    super::roster_sink(home, medulla::hub::stderr_log(), shared(Vec::new()))(&[
        worker("this-device", "this-device", false),
        worker("beta", "3Hob1Fxu", false),
    ]);
    assert_eq!(super::workers_from_config(home).len(), 2, "seeded");

    let session = medulla::auth::Credentials {
        base_url: "https://api.example".into(),
        jwt: "jwt".into(),
    };

    let config = super::build_hub_config_with_host(
        &env(&[]),
        home,
        medulla::hub::stderr_log(),
        Some(super::LocalDispatch {
            network: medulla::bridge::LocalBridgeNetwork::new(),
            hub_address: "medulla-orchestrator".to_string(),
            host_addresses: shared(vec!["this-device".to_string()]),
            // Hosting is off: nothing is bound at `this-device` this run.
            hosts: Vec::new(),
        }),
        Some(&session),
        Vec::new(),
    )
    .expect("the hub config builds with a session present");

    assert!(
        !config.workers.iter().any(|w| w.address == "this-device"),
        "the stale local entry must not be advertised: {:?}",
        config
            .workers
            .iter()
            .map(|w| &w.address)
            .collect::<Vec<_>>()
    );
    assert_eq!(config.workers.len(), 1);
    assert_eq!(config.workers[0].address, "3Hob1Fxu");
}

#[test]
fn a_host_added_after_launch_is_not_remembered_as_a_remote_peer() {
    // The sink filters at *save* time, so a launch-time snapshot of the local
    // addresses did not know about a host started mid-session through
    // `LocalHostSpawner`. Its device-local entry was written into the saved
    // roster and would be advertised on a later run at an address nothing binds.
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path();

    let addresses = shared(vec!["this-device".to_string()]);
    let sink = super::roster_sink(home, medulla::hub::stderr_log(), addresses.clone());

    // The spawner binds a second host and appends its address.
    addresses
        .lock()
        .expect("host addresses")
        .push("local-backend".to_string());

    sink(&[
        worker("this-device", "this-device", false),
        worker("local-backend", "local-backend", false),
        worker("beta", "3Hob1Fxu", false),
    ]);

    let saved = super::workers_from_config(home);
    let addresses: Vec<&str> = saved.iter().map(|w| w.address.as_str()).collect();
    assert_eq!(
        addresses,
        vec!["3Hob1Fxu"],
        "only the genuinely remote peer is remembered"
    );
}
