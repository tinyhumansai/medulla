//! How the `[host]` section becomes start-up options, and which
//! misconfigurations are refused rather than silently widened.

use std::collections::HashMap;

use medulla::config::HostSection;
use medulla::tinyplace::HarnessProvider;

use crate::local_host::{
    host_address, host_enabled, options_from_config, options_from_config_with_custom,
};

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
        providers: vec!["codex".to_string()],
        default_provider: "codex".to_string(),
        concurrency: 5,
        task_timeout_ms: 1234,
        model: " sonnet ".to_string(),
        skip_permissions: false,
        ..HostSection::default()
    };

    let options = options_from_config(&config, &HashMap::new(), None, None, None, true)
        .expect("every name in this config is valid");

    assert_eq!(options.providers, Some(vec![HarnessProvider::Codex]));
    assert_eq!(options.default_provider, Some(HarnessProvider::Codex));
    assert_eq!(options.concurrency, 5);
    assert_eq!(options.task_timeout_ms, 1234);
    assert_eq!(options.model.as_deref(), Some("sonnet"));
    assert!(!options.skip_permissions);
}

#[test]
fn an_empty_provider_list_means_detect_rather_than_serve_nothing() {
    let options = options_from_config(
        &HostSection::default(),
        &HashMap::new(),
        None,
        None,
        None,
        true,
    )
    .expect("the default section is valid");
    assert_eq!(options.providers, None);
    assert_eq!(options.default_provider, None);
    assert_eq!(options.model, None);
}

#[test]
fn a_custom_harness_is_attached_only_to_its_fleet_host() {
    let mut local = medulla::config::CustomHarnessConfig::from_editor_line(
        "deepseek | DeepSeek | codex | deepseek/deepseek-chat | | this-device",
    )
    .expect("valid custom harness");
    local.default = true;
    let remote = medulla::config::CustomHarnessConfig::from_editor_line(
        "remote | Remote | claude | anthropic/claude-sonnet | | other-device",
    )
    .expect("valid custom harness");

    let options = options_from_config_with_custom(
        &HostSection::default(),
        &HashMap::new(),
        None,
        None,
        None,
        &[local.clone(), remote],
        true,
    )
    .expect("valid host options");

    assert_eq!(options.providers, Some(vec![HarnessProvider::Codex]));
    assert_eq!(options.default_provider, Some(HarnessProvider::Codex));
    assert_eq!(options.custom_harnesses, vec![local]);
}

#[test]
fn custom_harness_matching_uses_the_effective_host_address() {
    let local = medulla::config::CustomHarnessConfig::from_editor_line(
        "deepseek | DeepSeek | codex | deepseek/deepseek-chat | | this-device",
    )
    .expect("valid custom harness");
    let config = HostSection {
        address: "  ".into(),
        ..HostSection::default()
    };

    let options = options_from_config_with_custom(
        &config,
        &HashMap::new(),
        None,
        None,
        None,
        std::slice::from_ref(&local),
        true,
    )
    .expect("valid host options");

    assert_eq!(options.custom_harnesses, vec![local]);
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
        true,
    )
    .expect("a zero concurrency is clamped, not rejected");
    assert_eq!(options.concurrency, 1);
}

#[test]
fn an_unknown_harness_name_is_rejected_rather_than_silently_widening_the_host() {
    // A typo in `providers` used to parse to an empty list, and an empty list
    // means "detect everything installed" — so an entry meant to narrow what
    // this machine runs would instead widen it, and unattended work would go to
    // a CLI nobody chose with permission prompts bypassed.
    let error = options_from_config(
        &HostSection {
            providers: vec!["claudde".to_string()],
            ..HostSection::default()
        },
        &HashMap::new(),
        None,
        None,
        None,
        true,
    )
    .err()
    .expect("an unknown harness name is an error");

    assert!(error.contains("claudde"), "should name the typo: {error}");
    assert!(
        error.contains("claude, codex, opencode"),
        "should name the valid spellings: {error}"
    );
}

#[test]
fn an_unknown_default_harness_is_rejected_rather_than_falling_back() {
    // Falling back to "whichever was detected first" is the same failure in a
    // quieter form: the operator named one CLI and would silently get another.
    let error = options_from_config(
        &HostSection {
            default_provider: "clade".to_string(),
            ..HostSection::default()
        },
        &HashMap::new(),
        None,
        None,
        None,
        true,
    )
    .err()
    .expect("an unknown default harness is an error");

    assert!(error.contains("clade"), "should name the typo: {error}");
}

/// `EmbeddedDaemonOptions` defaults attribution to on, so the opt-out has to be
/// passed in explicitly — relying on `..Default::default()` left every embedded
/// host attributing commits despite `[attribution] commit = false`.
#[test]
fn the_attribution_opt_out_reaches_embedded_host_options() {
    let off = options_from_config(
        &HostSection::default(),
        &HashMap::new(),
        None,
        None,
        None,
        false,
    )
    .expect("the default section is valid");
    assert!(
        !off.attribution,
        "an operator's opt-out must reach the embedded host"
    );

    let on = options_from_config(
        &HostSection::default(),
        &HashMap::new(),
        None,
        None,
        None,
        true,
    )
    .expect("the default section is valid");
    assert!(on.attribution, "the default stays on");
}
