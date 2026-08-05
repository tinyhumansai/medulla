//! Tests for capability settings and the config that produces them.
//!
//! These pin the deliberate split in defaults: local code is available, while
//! network and third-party tools remain closed until an operator allowlists
//! them.

use std::path::Path;

use super::{CapabilitySettings, DEFAULT_MAX_PARALLEL_AGENTS, DEFAULT_RUN_TIMEOUT_SECS};
use crate::config::WorkflowsConfig;

#[test]
fn code_is_on_while_external_capabilities_stay_off_by_default() {
    let settings = CapabilitySettings::rooted_at(Path::new("/home"));

    assert!(settings.allow_code);
    assert!(settings.tool_allowlist.is_empty());
    assert!(settings.http_allowlist.is_empty());
    assert!(
        !settings.http_host_allowed("example.com"),
        "an empty allowlist permits nothing, rather than everything"
    );
    assert!(!settings.tool_allowed("github.create_issue"));
}

#[test]
fn an_explicit_config_opt_out_disables_code_execution() {
    let config = WorkflowsConfig {
        allow_code: false,
        ..Default::default()
    };

    let settings = CapabilitySettings::from_config(&config, "/home");

    assert!(!settings.allow_code);
}

#[test]
fn unreadable_config_fallback_disables_code_execution() {
    let settings = CapabilitySettings::fail_closed_at("/home");

    assert!(!settings.allow_code);
    assert!(settings.tool_allowlist.is_empty());
    assert!(settings.http_allowlist.is_empty());
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
        shell: "zsh".into(),
        shell_args: vec!["-l".into()],
        tool_allowlist: vec!["github.create_issue".into()],
        http_allowlist: vec!["api.github.com".into()],
        run_timeout_secs: 30,
        max_parallel_agents: 6,
        max_loop_iterations: 12,
        evolve: Default::default(),
    };

    let settings = CapabilitySettings::from_config(&config, "/home/.medulla");

    assert_eq!(settings.default_worker_address, "builder");
    assert_eq!(settings.default_model.as_deref(), Some("some-model"));
    assert!(settings.allow_code);
    assert!(settings.tool_allowed("github.create_issue"));
    assert!(settings.http_host_allowed("api.github.com"));
    assert_eq!(settings.run_timeout_secs, 30);
    assert_eq!(settings.max_parallel_agents, 6);
    assert_eq!(settings.max_loop_iterations, 12);
}

#[test]
fn a_zero_loop_ceiling_falls_back_rather_than_clamping_every_loop_to_nothing() {
    // Taking a zero literally would clamp every loop to no iterations at all,
    // so a workflow that looks correct would quietly do nothing.
    let config = WorkflowsConfig {
        max_loop_iterations: 0,
        ..Default::default()
    };
    let settings = CapabilitySettings::from_config(&config, "/home/.medulla");
    assert_eq!(
        settings.max_loop_iterations,
        crate::flow_engine::DEFAULT_MAX_LOOP_ITERATIONS
    );
}

#[test]
fn a_zero_agent_ceiling_falls_back_rather_than_wedging_every_agent_node() {
    // A zero-permit semaphore would leave every agent node awaiting a permit
    // that can never arrive, which presents as a hang rather than as a
    // configuration mistake.
    let config = WorkflowsConfig {
        max_parallel_agents: 0,
        evolve: Default::default(),
        ..Default::default()
    };

    let settings = CapabilitySettings::from_config(&config, "/home");

    assert_eq!(settings.max_parallel_agents, DEFAULT_MAX_PARALLEL_AGENTS);
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

#[test]
fn the_configured_shell_and_its_arguments_reach_the_settings() {
    let config = WorkflowsConfig {
        shell: "zsh".into(),
        shell_args: vec!["-l".into()],
        ..Default::default()
    };

    let settings = CapabilitySettings::from_config(&config, "/home/.medulla");

    assert_eq!(settings.shell.program, "zsh");
    assert_eq!(settings.shell.args, vec!["-l".to_string()]);
}

#[test]
fn an_unconfigured_host_gets_the_default_shell() {
    let settings = CapabilitySettings::from_config(&WorkflowsConfig::default(), "/home/.medulla");

    assert_eq!(
        settings.shell,
        crate::flow_engine::caps::script::Interpreter::default_shell()
    );
}

#[test]
fn an_unusable_shell_setting_falls_back_rather_than_breaking_every_script() {
    // A configuration mistake here must not take out `code` nodes too, none of
    // which use this shell — so it degrades to the default and warns.
    let config = WorkflowsConfig {
        shell: "./not-absolute".into(),
        ..Default::default()
    };

    let settings = CapabilitySettings::from_config(&config, "/home/.medulla");

    assert_eq!(
        settings.shell,
        crate::flow_engine::caps::script::Interpreter::default_shell()
    );
}
