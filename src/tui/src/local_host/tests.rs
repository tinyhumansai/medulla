//! Unit tests for the local-host wiring: the on/off decision, how the `[host]`
//! section becomes start-up options, and what the hub is told to advertise.

use std::collections::HashMap;

use medulla::bridge::LocalBridgeNetwork;
use medulla::config::HostSection;
use medulla::tinyplace::HarnessProvider;

use super::{host_address, host_enabled, options_from_config, start};

/// A path that is guaranteed to exist and be executable on every platform.
///
/// The running test binary itself. `/bin/sh` was the obvious choice and the
/// wrong one: it does not exist on Windows, so detection found nothing there and
/// every test that needed an "installed" harness failed on that runner alone.
fn installed_bin() -> String {
    std::env::current_exe()
        .expect("the test binary has a path")
        .to_string_lossy()
        .into_owned()
}

/// An env with exactly `claude` "installed", so detection is deterministic and
/// independent of what the machine running the tests actually has.
fn env_with_only_claude() -> HashMap<String, String> {
    HashMap::from([
        ("PATH".to_string(), String::new()),
        ("TINYPLACE_CLAUDE_BIN".to_string(), installed_bin()),
    ])
}

#[test]
fn hosting_is_on_by_default_and_the_env_overrides_the_config_either_way() {
    let on = HostSection::default();
    let off = HostSection {
        enabled: false,
        ..HostSection::default()
    };
    let empty = HashMap::new();

    assert!(host_enabled(&on, &empty));
    assert!(!host_enabled(&off, &empty));
    assert!(!host_enabled(
        &on,
        &HashMap::from([("MEDULLA_HOST".to_string(), "0".to_string())])
    ));
    assert!(host_enabled(
        &off,
        &HashMap::from([("MEDULLA_HOST".to_string(), "1".to_string())])
    ));
    // A blank value is not a decision, so the config still wins.
    assert!(host_enabled(
        &on,
        &HashMap::from([("MEDULLA_HOST".to_string(), "  ".to_string())])
    ));
}

#[test]
fn a_blanked_address_falls_back_to_the_documented_one_rather_than_failing_to_bind() {
    assert_eq!(host_address(&HostSection::default()), "this-device");
    assert_eq!(
        host_address(&HostSection {
            address: "   ".to_string(),
            ..HostSection::default()
        }),
        "this-device"
    );
    assert_eq!(
        host_address(&HostSection {
            address: " laptop ".to_string(),
            ..HostSection::default()
        }),
        "laptop"
    );
}

#[test]
fn the_config_section_maps_onto_start_up_options() {
    let config = HostSection {
        providers: vec!["codex".to_string(), "nonsense".to_string()],
        default_provider: "codex".to_string(),
        concurrency: 5,
        task_timeout_ms: 1234,
        model: " sonnet ".to_string(),
        skip_permissions: false,
        ..HostSection::default()
    };

    let options = options_from_config(&config, &HashMap::new(), None, None, None);

    // The unparseable entry is dropped, not fatal, and not silently kept.
    assert_eq!(options.providers, Some(vec![HarnessProvider::Codex]));
    assert_eq!(options.default_provider, Some(HarnessProvider::Codex));
    assert_eq!(options.concurrency, 5);
    assert_eq!(options.task_timeout_ms, 1234);
    assert_eq!(options.model.as_deref(), Some("sonnet"));
    assert!(!options.skip_permissions);
}

#[test]
fn an_empty_provider_list_means_detect_rather_than_serve_nothing() {
    let options = options_from_config(&HostSection::default(), &HashMap::new(), None, None, None);
    assert_eq!(options.providers, None);
    assert_eq!(options.default_provider, None);
    assert_eq!(options.model, None);
}

#[test]
fn a_zero_concurrency_config_still_runs_one_task_at_a_time() {
    let options = options_from_config(
        &HostSection {
            concurrency: 0,
            ..HostSection::default()
        },
        &HashMap::new(),
        None,
        None,
        None,
    );
    assert_eq!(options.concurrency, 1);
}

#[tokio::test]
async fn hosting_switched_off_is_a_choice_not_an_error() {
    let network = LocalBridgeNetwork::new();
    let config = HostSection {
        enabled: false,
        ..HostSection::default()
    };
    let options = options_from_config(&config, &env_with_only_claude(), None, None, None);

    let host = start(&config, &HashMap::new(), &network, options).unwrap();

    assert!(host.is_none());
    // Nothing was bound, so the address is still free for a later run.
    assert!(network.bind("this-device").is_ok());
}

#[tokio::test]
async fn a_started_host_advertises_this_machine_to_the_hub() {
    let network = LocalBridgeNetwork::new();
    let config = HostSection::default();
    let env = env_with_only_claude();
    let options = options_from_config(&config, &env, None, None, None);

    let host = start(&config, &HashMap::new(), &network, options)
        .unwrap()
        .expect("hosting is on by default");

    assert_eq!(host.address(), "this-device");
    assert_eq!(host.providers(), [HarnessProvider::Claude]);
    let spec = host.spec();
    assert_eq!(spec.address, "this-device");
    assert_eq!(spec.name, "this device");
    assert_eq!(spec.harness, "claude");
    assert!(
        spec.description.contains("claude") && spec.description.contains(host.workspace()),
        "the roster entry should say what runs where: {}",
        spec.description
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
        options_from_config(&config, &env, None, None, None),
    )
    .unwrap()
    .expect("the first host starts");

    let error = start(
        &config,
        &HashMap::new(),
        &network,
        options_from_config(&config, &env, None, None, None),
    )
    .unwrap_err();

    assert!(
        error.contains("could not host on this device"),
        "unexpected error: {error}"
    );
}
