//! Unit tests for the local-host wiring: the on/off decision, how the `[host]`
//! section becomes start-up options, and what the hub is told to advertise.

use std::collections::HashMap;

use medulla::bridge::LocalBridgeNetwork;
use medulla::config::HostSection;
use medulla::daemon::providers::{Abort, RunTaskOptions};
use medulla::tinyplace::HarnessProvider;
use medulla_tui::worker::executor::PtySessionExecutor;
use medulla_tui::worker::pty::PtyManager;

use super::{host_address, host_enabled, options_from_config, run_task, start};

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
