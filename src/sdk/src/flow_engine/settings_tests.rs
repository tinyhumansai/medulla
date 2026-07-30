//! Tests for capability settings and the config that produces them.
//!
//! These are mostly about defaults being *closed*: a workflow arriving as a file
//! must not be able to reach the network, run code, or call a third-party tool
//! until an operator says so.

use std::path::Path;

use super::{CapabilitySettings, DEFAULT_RUN_TIMEOUT_SECS};
use crate::config::WorkflowsConfig;

#[test]
fn every_optional_capability_is_off_by_default() {
    let settings = CapabilitySettings::rooted_at(Path::new("/home"));

    assert!(!settings.allow_code, "no sandbox means no code execution");
    assert!(settings.tool_allowlist.is_empty());
    assert!(settings.http_allowlist.is_empty());
    assert!(
        !settings.http_host_allowed("example.com"),
        "an empty allowlist permits nothing, rather than everything"
    );
    assert!(!settings.tool_allowed("github.create_issue"));
}

#[test]
fn state_and_checkpoints_live_under_the_medulla_home() {
    let settings = CapabilitySettings::rooted_at(Path::new("/home/.medulla"));

    assert!(settings
        .state_dir
        .starts_with("/home/.medulla/state/workflows"));
    assert!(settings
        .checkpoint_dir
        .starts_with("/home/.medulla/state/workflows"));
    assert_ne!(
        settings.state_dir, settings.checkpoint_dir,
        "run checkpoints and workflow state are different things"
    );
}

#[test]
fn an_allowlisted_domain_covers_its_subdomains_but_not_a_lookalike() {
    let mut settings = CapabilitySettings::rooted_at(Path::new("/home"));
    settings.http_allowlist = vec!["example.com".into()];

    assert!(settings.http_host_allowed("example.com"));
    assert!(settings.http_host_allowed("api.example.com"));
    assert!(
        settings.http_host_allowed("API.Example.COM"),
        "case-insensitive"
    );
    assert!(
        !settings.http_host_allowed("notexample.com"),
        "a suffix match must not be a substring match"
    );
    assert!(!settings.http_host_allowed("example.com.evil.test"));
}

#[test]
fn config_becomes_settings_with_every_field_carried_across() {
    let config = WorkflowsConfig {
        enabled: true,
        default_worker: "builder".into(),
        default_provider: None,
        default_model: "some-model".into(),
        allow_code: true,
        tool_allowlist: vec!["github.create_issue".into()],
        http_allowlist: vec!["api.github.com".into()],
        run_timeout_secs: 30,
        evolve: Default::default(),
    };

    let settings = CapabilitySettings::from_config(&config, "/home/.medulla");

    assert_eq!(settings.default_worker_address, "builder");
    assert_eq!(settings.default_model.as_deref(), Some("some-model"));
    assert!(settings.allow_code);
    assert!(settings.tool_allowed("github.create_issue"));
    assert!(settings.http_host_allowed("api.github.com"));
    assert_eq!(settings.run_timeout_secs, 30);
}

#[test]
fn an_empty_default_model_becomes_no_hint_rather_than_an_empty_one() {
    let settings = CapabilitySettings::from_config(&WorkflowsConfig::default(), "/home");

    assert_eq!(
        settings.default_model, None,
        "an empty string would be sent as a model name and rejected"
    );
}

#[test]
fn a_zero_timeout_falls_back_to_the_default_rather_than_abandoning_every_run() {
    let config = WorkflowsConfig {
        run_timeout_secs: 0,
        evolve: Default::default(),
        ..Default::default()
    };

    let settings = CapabilitySettings::from_config(&config, "/home");

    assert_eq!(settings.run_timeout_secs, DEFAULT_RUN_TIMEOUT_SECS);
}

#[test]
fn the_shipped_config_default_matches_the_seams_own_default_timeout() {
    // Two constants for one number is a drift risk; this is the guard.
    assert_eq!(
        WorkflowsConfig::default().run_timeout_secs,
        DEFAULT_RUN_TIMEOUT_SECS
    );
}

#[test]
fn a_short_run_timeout_never_gives_a_script_more_budget_than_the_run_itself() {
    // Regression: the quarter-share floor (30s) used to apply unconditionally,
    // so any run shorter than 120s handed its scripts a *bigger* budget than
    // the run's own — the opposite of "a wedged script fails as a script
    // before it can consume the run's whole budget."
    let mut settings = CapabilitySettings::rooted_at("/home");
    settings.run_timeout_secs = 20;

    assert_eq!(
        settings.script_timeout(),
        std::time::Duration::from_secs(20),
        "a script must never outlive the run that bounds it"
    );
}

#[test]
fn a_generous_run_timeout_still_gives_a_script_only_a_quarter_share() {
    let mut settings = CapabilitySettings::rooted_at("/home");
    settings.run_timeout_secs = 400;

    assert_eq!(
        settings.script_timeout(),
        std::time::Duration::from_secs(100)
    );
}

#[test]
fn a_mid_sized_run_timeout_still_gets_the_thirty_second_floor() {
    // Below 120s the quarter-share is under the 30s floor, but the floor
    // itself must stay under the run's own budget (see the short-timeout case
    // above) — this pins the boundary where the floor applies uncapped.
    let mut settings = CapabilitySettings::rooted_at("/home");
    settings.run_timeout_secs = 60;

    assert_eq!(
        settings.script_timeout(),
        std::time::Duration::from_secs(30)
    );
}
