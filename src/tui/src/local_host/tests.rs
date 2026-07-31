//! Unit tests for the local-host wiring: the on/off decision, how the `[host]`
//! section becomes start-up options, and what the hub is told to advertise.

use std::collections::HashMap;

use medulla::bridge::LocalBridgeNetwork;
use medulla::config::HostSection;
use medulla::daemon::embedded::EmbeddedDaemonOptions;
use medulla::daemon::providers::{Abort, RunTaskOptions};
use medulla::tinyplace::HarnessProvider;
use medulla_tui::worker::executor::PtySessionExecutor;
use medulla_tui::worker::pty::PtyManager;

use super::{
    all_host_addresses, display_name, extra_host_address, extra_options, host_address,
    host_enabled, options_from_config, options_from_config_with_custom, run_task, start, start_all,
};

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
        providers: vec!["codex".to_string()],
        default_provider: "codex".to_string(),
        concurrency: 5,
        task_timeout_ms: 1234,
        model: " sonnet ".to_string(),
        skip_permissions: false,
        ..HostSection::default()
    };

    let options = options_from_config(&config, &HashMap::new(), None, None, None)
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
    let options = options_from_config(&HostSection::default(), &HashMap::new(), None, None, None)
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
    )
    .expect("a zero concurrency is clamped, not rejected");
    assert_eq!(options.concurrency, 1);
}

#[tokio::test]
async fn hosting_switched_off_is_a_choice_not_an_error() {
    let network = LocalBridgeNetwork::new();
    let config = HostSection {
        enabled: false,
        ..HostSection::default()
    };
    let options = options_from_config(&config, &env_with_only_claude(), None, None, None)
        .expect("valid config");

    let host = start(
        &config,
        &HashMap::new(),
        &network,
        options,
        PtyManager::new(),
    )
    .unwrap();

    assert!(host.is_none());
    // Nothing was bound, so the address is still free for a later run.
    assert!(network.bind("this-device").is_ok());
}

#[tokio::test]
async fn a_started_host_advertises_this_machine_to_the_hub() {
    let network = LocalBridgeNetwork::new();
    let config = HostSection::default();
    let env = env_with_only_claude();
    let options = options_from_config(&config, &env, None, None, None).expect("valid config");

    let host = start(
        &config,
        &HashMap::new(),
        &network,
        options,
        PtyManager::new(),
    )
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
    // Structured, not only prose: the backend places the agent from
    // `metadata.workspace`, and a path buried in the description does not
    // reach it — the orchestrator would see a host with no workspace at all.
    assert_eq!(spec.workspace.as_deref(), Some(host.workspace()));
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
        options_from_config(&config, &env, None, None, None).expect("valid config"),
        PtyManager::new(),
    )
    .unwrap()
    .expect("the first host starts");

    let error = start(
        &config,
        &HashMap::new(),
        &network,
        options_from_config(&config, &env, None, None, None).expect("valid config"),
        PtyManager::new(),
    )
    .unwrap_err();

    assert!(
        error.contains("could not host on this device"),
        "unexpected error: {error}"
    );
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
    )
    .err()
    .expect("an unknown default harness is an error");

    assert!(error.contains("clade"), "should name the typo: {error}");
}

/// Bare-bones `RunTaskOptions` for a dispatch test — no callbacks, no
/// conversation, just enough to reach the executor the dispatcher picks.
fn dispatch_options(provider: HarnessProvider, bin_env_key: &str) -> RunTaskOptions {
    RunTaskOptions {
        provider,
        prompt: "hi".to_string(),
        cwd: ".".to_string(),
        env: HashMap::from([(
            bin_env_key.to_string(),
            "/definitely/does/not/exist-medulla-test".to_string(),
        )]),
        timeout_ms: 5_000,
        model: None,
        agent: None,
        extra_args: Vec::new(),
        skip_permissions: false,
        conversation: String::new(),
        session_class: medulla::sessions::SessionClass::Bounded,
        resume_session_id: None,
        abort: Abort::new(),
        router: None,
        on_event: None,
        on_stdin: None,
        on_session: None,
        attribution: true,
    }
}

#[tokio::test]
async fn opencode_falls_back_to_the_headless_executor_rather_than_being_refused() {
    // The regression: switching the local host to `PtySessionExecutor` made
    // every OpenCode task fail with "cannot run watchable tasks", because that
    // executor refuses a provider it has no transcript to tail for — even
    // though OpenCode is fully capable of running headlessly, exactly as it did
    // before this host existed. The dispatcher must route it there instead of
    // leaving it stranded behind the pty refusal.
    let run = run_task(PtySessionExecutor::new(
        PtyManager::new(),
        HashMap::new(),
        ".".to_string(),
    ));

    let error = run(dispatch_options(
        HarnessProvider::Opencode,
        "TINYPLACE_OPENCODE_BIN",
    ))
    .await
    .expect_err("a nonexistent binary must fail to spawn");

    assert!(
        !error.contains("cannot run watchable tasks"),
        "opencode must never hit the pty executor's refusal: {error}"
    );
    // The headless executor's own failure-to-spawn message, proving the task
    // actually reached `run_provider_task` rather than merely avoiding the pty
    // one for an unrelated reason.
    assert!(
        error.contains("failed to start"),
        "expected the headless executor's spawn error, got: {error}"
    );
}

#[tokio::test]
async fn claude_and_codex_still_reach_the_pty_executor() {
    // The other half of the dispatch: providers the pty executor *can* tail
    // must still go through it, not accidentally fall through to headless.
    let run = run_task(PtySessionExecutor::new(
        PtyManager::new(),
        HashMap::new(),
        ".".to_string(),
    ));

    for provider in [HarnessProvider::Claude, HarnessProvider::Codex] {
        let error = run(dispatch_options(provider, "irrelevant"))
            .await
            .expect_err("no real harness is installed in this test env");
        assert!(
            !error.contains("failed to start"),
            "{provider:?} must not take the headless path: {error}"
        );
    }
}

#[test]
fn each_extra_host_gets_its_own_bus_address() {
    // Two hosts cannot share an address — the second `bind` fails — so an
    // operator who declares `[[hosts]]` without thinking about addressing would
    // otherwise get one working host and one startup error.
    let named = HostSection {
        name: "Backend API".to_string(),
        ..HostSection::default()
    };
    let anonymous = HostSection {
        address: String::new(),
        name: String::new(),
        ..HostSection::default()
    };
    let explicit = HostSection {
        address: "chosen-by-hand".to_string(),
        ..HostSection::default()
    };

    assert_eq!(extra_host_address(&named, 0), "local-backend-api");
    assert_eq!(extra_host_address(&anonymous, 3), "local-host-4");
    assert_eq!(extra_host_address(&explicit, 0), "chosen-by-hand");
}

#[test]
fn every_declared_address_is_known_without_starting_anything() {
    // Needed in exactly the case where none started: a roster saved while
    // hosting was on must not keep advertising local entries nothing answers.
    let primary = HostSection::default();
    let extras = [
        HostSection {
            name: "backend".to_string(),
            ..HostSection::default()
        },
        HostSection {
            address: "custom".to_string(),
            ..HostSection::default()
        },
    ];
    assert_eq!(
        all_host_addresses(&primary, &extras),
        vec![
            HostSection::default().address,
            "local-backend".to_string(),
            "custom".to_string()
        ]
    );
}

#[test]
fn an_unnamed_extra_is_named_for_the_directory_it_works_in() {
    // Several hosts on one machine differ only by where they work, so "this
    // device" repeated would describe none of them.
    let unnamed = HostSection {
        name: String::new(),
        ..HostSection::default()
    };
    assert_eq!(
        display_name(&unnamed, "/Users/me/Projects/backend", false),
        "backend"
    );
    // The primary keeps its name: it *is* the machine the operator is at.
    assert_eq!(
        display_name(&unnamed, "/Users/me/Projects/backend", true),
        "this device"
    );
    // An explicit name always wins.
    let named = HostSection {
        name: "API box".to_string(),
        ..HostSection::default()
    };
    assert_eq!(display_name(&named, "/anywhere", false), "API box");
}

#[tokio::test]
async fn a_failing_extra_does_not_take_the_other_hosts_down() {
    // One mistyped directory should cost that host, not hosting altogether.
    let env = env_with_only_claude();
    let network = LocalBridgeNetwork::new();
    let primary = HostSection::default();
    // Two extras that collide on one address: the second cannot bind.
    let extras = [
        HostSection {
            address: "duplicate".to_string(),
            ..HostSection::default()
        },
        HostSection {
            address: "duplicate".to_string(),
            ..HostSection::default()
        },
    ];
    let options = options_from_config(&primary, &env, None, None, None).expect("options");

    let (hosts, problems) = start_all(
        &primary,
        &extras,
        &env,
        &network,
        options,
        PtyManager::new(),
    );

    assert_eq!(hosts.len(), 2, "the primary and the first extra both start");
    assert_eq!(
        problems.len(),
        1,
        "and the collision is reported, not fatal"
    );
}

#[test]
fn an_extra_runs_the_harness_it_declared_rather_than_the_primarys() {
    // The Add Host wizard asks which harness a local host runs and writes the
    // answer to `providers`/`defaultProvider`. Inheriting the primary's meant
    // the answer was persisted and then never read — a host added as codex ran
    // claude because the primary did.
    let primary = EmbeddedDaemonOptions {
        providers: Some(vec![HarnessProvider::Claude]),
        default_provider: Some(HarnessProvider::Claude),
        workspace: "/primary".to_string(),
        ..Default::default()
    };
    let extra = HostSection {
        workspace: "/extra".to_string(),
        providers: vec!["codex".to_string()],
        default_provider: "codex".to_string(),
        ..HostSection::default()
    };

    let options = extra_options(&primary, &extra).expect("valid providers");
    assert_eq!(options.default_provider, Some(HarnessProvider::Codex));
    assert_eq!(options.providers, Some(vec![HarnessProvider::Codex]));
    assert_eq!(options.workspace, "/extra");
}

#[test]
fn an_extra_that_names_no_harness_still_inherits_the_primarys() {
    // A `[[hosts]]` entry that is only a directory keeps behaving as one.
    let primary = EmbeddedDaemonOptions {
        providers: Some(vec![HarnessProvider::Claude]),
        default_provider: Some(HarnessProvider::Claude),
        ..Default::default()
    };
    let extra = HostSection {
        workspace: "/extra".to_string(),
        ..HostSection::default()
    };

    let options = extra_options(&primary, &extra).expect("no providers is fine");
    assert_eq!(options.default_provider, Some(HarnessProvider::Claude));
    assert_eq!(options.providers, Some(vec![HarnessProvider::Claude]));
}

#[test]
fn an_extras_unknown_harness_is_rejected_rather_than_silently_widening() {
    // Same rule the primary follows: an empty provider list means "detect
    // everything installed", so dropping a typo would widen what this machine
    // runs when the operator meant to narrow it.
    let extra = HostSection {
        workspace: "/extra".to_string(),
        providers: vec!["claudde".to_string()],
        ..HostSection::default()
    };
    assert!(extra_options(&EmbeddedDaemonOptions::default(), &extra).is_err());
}

#[test]
fn an_unnamed_extras_address_is_derived_from_its_config_index() {
    // `spawn` used to count *started* hosts, which includes the primary, so a
    // first unnamed extra bound `local-host-2` this run and `local-host-1` on
    // the next launch — leaving the roster remembering an address nothing binds.
    // Every site now derives from the entry's position within `[[hosts]]`.
    let unnamed = HostSection {
        workspace: "/extra".to_string(),
        address: String::new(),
        ..HostSection::default()
    };
    let primary = HostSection::default();

    assert_eq!(extra_host_address(&unnamed, 0), "local-host-1");
    assert_eq!(
        all_host_addresses(&primary, std::slice::from_ref(&unnamed)),
        vec![host_address(&primary), "local-host-1".to_string()],
    );
}

#[test]
fn an_extra_that_replaces_the_provider_list_drops_a_default_outside_it() {
    // Provider-only is a valid configuration, and inheriting the primary's
    // default there names a harness this host was just told not to run — the
    // same wrong-harness outcome, reached without a typo.
    let primary = EmbeddedDaemonOptions {
        providers: Some(vec![HarnessProvider::Claude]),
        default_provider: Some(HarnessProvider::Claude),
        ..Default::default()
    };
    let extra = HostSection {
        workspace: "/extra".to_string(),
        providers: vec!["codex".to_string()],
        default_provider: String::new(),
        ..HostSection::default()
    };

    let options = extra_options(&primary, &extra).expect("valid providers");
    assert_eq!(options.providers, Some(vec![HarnessProvider::Codex]));
    assert_eq!(
        options.default_provider, None,
        "the inherited default is not in the new list, so it is not a default"
    );
}

#[test]
fn an_inherited_default_survives_when_the_new_list_still_allows_it() {
    // Widening rather than replacing: claude is still permitted, so the
    // primary's default is still a sensible answer and clearing it would make
    // the host pick arbitrarily.
    let primary = EmbeddedDaemonOptions {
        providers: Some(vec![HarnessProvider::Claude]),
        default_provider: Some(HarnessProvider::Claude),
        ..Default::default()
    };
    let extra = HostSection {
        workspace: "/extra".to_string(),
        providers: vec!["claude".to_string(), "codex".to_string()],
        default_provider: String::new(),
        ..HostSection::default()
    };

    let options = extra_options(&primary, &extra).expect("valid providers");
    assert_eq!(options.default_provider, Some(HarnessProvider::Claude));
}
