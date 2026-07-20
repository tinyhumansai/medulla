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
