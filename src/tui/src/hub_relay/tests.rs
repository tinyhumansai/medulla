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
        id: id.to_string(),
        address: address.to_string(),
        harness: "claude".to_string(),
        label: Some("laptop".to_string()),
        selected,
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

    let sink = super::roster_sink(home, medulla::hub::stderr_log());
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
    super::roster_sink(home, medulla::hub::stderr_log())(&[worker("saved", "addr-saved", false)]);

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

    super::roster_sink(home, medulla::hub::stderr_log())(&[worker("alpha", "addr", false)]);

    let text = std::fs::read_to_string(home.join("config.toml")).expect("read");
    assert!(text.contains("welcomeCompleted"), "got: {text}");
    assert!(text.contains("addr"), "got: {text}");
}

#[test]
fn an_unwritable_roster_path_does_not_take_the_hub_down() {
    // Losing the roster is a nuisance; failing to start is an outage.
    let sink = super::roster_sink(
        std::path::Path::new("/proc/nonexistent/nope"),
        medulla::hub::stderr_log(),
    );
    sink(&[worker("alpha", "addr", false)]);
}
