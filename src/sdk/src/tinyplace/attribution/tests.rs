//! Unit tests for [`super::attribution`]: trailer shape, the kill-switch
//! precedence matrix, and per-provider coverage.

use std::collections::HashMap;

use super::{
    attribution_args, attribution_enabled, attribution_trailer, ATTRIBUTION_EMAIL,
    ATTRIBUTION_ENV_KEY, ATTRIBUTION_NAME,
};
use crate::tinyplace::HarnessProvider;

/// An env map with `TINYPLACE_GIT_ATTRIBUTION` set to `value`.
fn env_with(value: &str) -> HashMap<String, String> {
    HashMap::from([(ATTRIBUTION_ENV_KEY.to_string(), value.to_string())])
}

#[test]
fn trailer_uses_the_medulla_identity() {
    assert_eq!(
        attribution_trailer(),
        "Co-authored-by: Medulla <medulla@tinyhumans.ai>"
    );
    assert_eq!(ATTRIBUTION_NAME, "Medulla");
    assert_eq!(ATTRIBUTION_EMAIL, "medulla@tinyhumans.ai");
}

#[test]
fn attribution_is_enabled_by_default() {
    assert!(attribution_enabled(&HashMap::new()));
}

#[test]
fn kill_switch_disables_attribution() {
    for raw in ["0", "false", "no", "off", "", "  "] {
        assert!(
            !attribution_enabled(&env_with(raw)),
            "expected {raw:?} to disable attribution"
        );
    }
}

#[test]
fn affirmative_values_keep_attribution_on() {
    for raw in ["1", "true", "yes", "on", "TRUE", " On "] {
        assert!(
            attribution_enabled(&env_with(raw)),
            "expected {raw:?} to enable attribution"
        );
    }
}

/// An unrecognised value should fail closed rather than silently attributing.
#[test]
fn unrecognised_values_fail_closed() {
    assert!(!attribution_enabled(&env_with("maybe")));
}

#[test]
fn claude_receives_inline_settings_carrying_the_trailer() {
    let args = attribution_args(HarnessProvider::Claude, &HashMap::new());
    assert_eq!(args.len(), 2, "expected a flag/value pair, got {args:?}");
    assert_eq!(args[0], "--settings");

    let parsed: serde_json::Value =
        serde_json::from_str(&args[1]).expect("settings payload must be valid JSON");
    assert_eq!(
        parsed["attribution"]["commit"],
        serde_json::Value::String(attribution_trailer()),
    );
}

/// The payload must carry *only* `attribution.commit`, so it layers over the
/// operator's own settings without clobbering unrelated keys.
#[test]
fn claude_settings_payload_is_minimal() {
    let args = attribution_args(HarnessProvider::Claude, &HashMap::new());
    let parsed: serde_json::Value = serde_json::from_str(&args[1]).unwrap();

    let top = parsed.as_object().expect("payload is a JSON object");
    assert_eq!(top.len(), 1, "unexpected top-level keys: {top:?}");
    let attribution = parsed["attribution"]
        .as_object()
        .expect("attribution is a JSON object");
    assert_eq!(attribution.len(), 1, "unexpected keys: {attribution:?}");
}

/// Codex hardcodes its own trailer and Opencode has no knob at all, so Medulla
/// leaves both alone rather than misattributing them.
#[test]
fn providers_without_a_knob_receive_no_args() {
    for provider in [HarnessProvider::Codex, HarnessProvider::Opencode] {
        assert!(
            attribution_args(provider, &HashMap::new()).is_empty(),
            "{provider:?} should receive no attribution args"
        );
    }
}

#[test]
fn kill_switch_suppresses_args_for_every_provider() {
    let env = env_with("0");
    for provider in [
        HarnessProvider::Claude,
        HarnessProvider::Codex,
        HarnessProvider::Opencode,
    ] {
        assert!(
            attribution_args(provider, &env).is_empty(),
            "{provider:?} should receive no args when disabled"
        );
    }
}
