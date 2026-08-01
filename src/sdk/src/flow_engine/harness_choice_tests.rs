//! Unit tests for harness and model selection.

use serde_json::json;

use super::*;

fn preference(config: serde_json::Value) -> HarnessPreference {
    HarnessPreference::from_config(&config).expect("preference parses")
}

#[test]
fn builtin_names_parse_to_providers() {
    assert_eq!(
        HarnessSelector::parse("claude"),
        Ok(HarnessSelector::Builtin(HarnessProvider::Claude))
    );
    assert_eq!(
        HarnessSelector::parse("  Codex "),
        Ok(HarnessSelector::Builtin(HarnessProvider::Codex))
    );
    assert_eq!(
        HarnessSelector::parse("opencode"),
        Ok(HarnessSelector::Builtin(HarnessProvider::Opencode))
    );
}

#[test]
fn an_unknown_name_is_taken_as_a_custom_preset() {
    assert_eq!(
        HarnessSelector::parse("deepseek-claude"),
        Ok(HarnessSelector::Custom("deepseek-claude".to_string()))
    );
}

#[test]
fn an_empty_harness_names_what_is_accepted() {
    let err = HarnessSelector::parse("   ").expect_err("empty is refused");
    assert!(err.contains("claude"), "{err}");
    assert!(err.contains("codex"), "{err}");
}

#[test]
fn an_unusable_preset_id_is_refused() {
    let err = HarnessSelector::parse("claude code!").expect_err("spaces are refused");
    assert!(err.contains("custom harness id"), "{err}");
    let long = "a".repeat(MAX_CUSTOM_HARNESS_LEN + 1);
    let err = HarnessSelector::parse(&long).expect_err("over-long is refused");
    assert!(err.contains("at most"), "{err}");
}

#[test]
fn config_reads_harness_and_model() {
    let parsed = preference(json!({ "harness": "codex", "model": " gpt-5 " }));
    assert_eq!(
        parsed.harness,
        Some(HarnessSelector::Builtin(HarnessProvider::Codex))
    );
    assert_eq!(parsed.model.as_deref(), Some("gpt-5"));
}

#[test]
fn provider_is_accepted_as_an_alias_for_harness() {
    let parsed = preference(json!({ "provider": "claude" }));
    assert_eq!(
        parsed.harness,
        Some(HarnessSelector::Builtin(HarnessProvider::Claude))
    );
}

#[test]
fn harness_wins_over_the_provider_alias() {
    let parsed = preference(json!({ "harness": "codex", "provider": "claude" }));
    assert_eq!(
        parsed.harness,
        Some(HarnessSelector::Builtin(HarnessProvider::Codex))
    );
}

#[test]
fn blank_and_null_fields_say_nothing() {
    assert!(preference(json!({ "harness": "", "model": "  " })).is_empty());
    assert!(preference(json!({ "harness": null, "model": null })).is_empty());
    assert!(preference(json!({})).is_empty());
}

#[test]
fn a_non_string_harness_is_refused_by_type() {
    let err = HarnessPreference::from_config(&json!({ "harness": 7 }))
        .expect_err("a number is not a harness");
    assert!(err.contains("must be a string"), "{err}");
    assert!(err.contains("a number"), "{err}");
}

#[test]
fn the_most_specific_layer_wins() {
    let choice = HarnessChoice::resolve(&[
        preference(json!({ "harness": "codex", "model": "gpt-5" })),
        preference(json!({ "harness": "claude", "model": "opus" })),
    ]);
    assert_eq!(choice.provider, Some(HarnessProvider::Codex));
    assert_eq!(choice.model.as_deref(), Some("gpt-5"));
    assert_eq!(choice.custom_harness, None);
}

#[test]
fn a_node_that_switches_harness_does_not_inherit_the_lower_model() {
    // The whole point of paired resolution: a Claude model id must not ride
    // along to Codex just because the host pinned one.
    let choice = HarnessChoice::resolve(&[
        preference(json!({ "harness": "codex" })),
        preference(json!({ "harness": "claude", "model": "claude-opus-4" })),
    ]);
    assert_eq!(choice.provider, Some(HarnessProvider::Codex));
    assert_eq!(choice.model, None);
}

#[test]
fn a_node_may_pin_only_the_model_and_keep_the_inherited_harness() {
    let choice = HarnessChoice::resolve(&[
        preference(json!({ "model": "haiku" })),
        preference(json!({ "harness": "claude", "model": "opus" })),
    ]);
    assert_eq!(choice.provider, Some(HarnessProvider::Claude));
    assert_eq!(choice.model.as_deref(), Some("haiku"));
}

#[test]
fn with_no_harness_anywhere_every_layer_may_supply_the_model() {
    let choice = HarnessChoice::resolve(&[
        preference(json!({})),
        preference(json!({})),
        preference(json!({ "model": "host-default" })),
    ]);
    assert_eq!(choice.provider, None);
    assert_eq!(choice.custom_harness, None);
    assert_eq!(choice.model.as_deref(), Some("host-default"));
}

#[test]
fn a_custom_preset_resolves_onto_the_custom_harness_field() {
    let choice = HarnessChoice::resolve(&[preference(json!({ "harness": "deepseek-claude" }))]);
    assert_eq!(choice.provider, None);
    assert_eq!(choice.custom_harness.as_deref(), Some("deepseek-claude"));
}

#[test]
fn resolving_nothing_chooses_nothing() {
    assert_eq!(HarnessChoice::resolve(&[]), HarnessChoice::default());
}
