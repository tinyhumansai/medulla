//! Tests for the main module.

use super::daemon_uses_tui;

#[test]
fn daemon_defaults_to_the_tui_only_on_a_terminal() {
    assert!(daemon_uses_tui(true, &["daemon".into()]));
    assert!(!daemon_uses_tui(false, &["daemon".into()]));
    assert!(!daemon_uses_tui(
        true,
        &["daemon".into(), "--headless".into()]
    ));
}

#[test]
fn the_telemetry_config_path_comes_from_the_selected_commands_parser() {
    let argv = |args: &[&str]| args.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let config = |raw: &[String]| super::config_source(raw, false).config;
    // `run` takes only `--config <path>`; `--config=…` is instruction text.
    assert_eq!(
        config(&argv(&["run", "set", "--config=/tmp/other.toml"])),
        None
    );
    assert_eq!(
        config(&argv(&["run", "--config", "/tmp/a.toml", "go"])).as_deref(),
        Some("/tmp/a.toml")
    );
    // A wrapper's flags belong to the child harness.
    assert_eq!(config(&argv(&["codex", "--config", "model=o3"])), None);
    assert_eq!(
        config(&argv(&["--config", "/tmp/t.toml"])).as_deref(),
        Some("/tmp/t.toml")
    );
    // `remote` and `daemon --direct` take the joined spelling too.
    assert_eq!(
        config(&argv(&["remote", "--config=/tmp/r.toml"])).as_deref(),
        Some("/tmp/r.toml")
    );
    // Only those commands take the joined spelling; elsewhere it is not a
    // config path.
    assert_eq!(
        config(&argv(&["workflow", "run", "x", "--config=/tmp/b.toml"])),
        None
    );
    assert_eq!(
        config(&argv(&["workflow", "run", "x", "--config", "/tmp/b.toml"])).as_deref(),
        Some("/tmp/b.toml")
    );
    #[cfg(feature = "workflows")]
    assert_eq!(
        config(&argv(&["mcp", "--config=/tmp/m.toml"])).as_deref(),
        Some("/tmp/m.toml")
    );
}

#[test]
fn the_daemon_resolves_telemetry_config_the_way_each_of_its_modes_does() {
    let argv = |args: &[&str]| args.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    // The daemon TUI's parser takes the joined spelling and discovers from cwd.
    let tui = super::config_source(&argv(&["daemon", "--config=/tmp/b.toml"]), true);
    assert_eq!(tui.config.as_deref(), Some("/tmp/b.toml"));
    assert_eq!(tui.dir, std::env::current_dir().unwrap());
    // `daemon --tui` is its own command, read by the same parser.
    let explicit = super::config_source(&argv(&["daemon", "--tui", "--config=/tmp/b.toml"]), false);
    assert_eq!(explicit.config.as_deref(), Some("/tmp/b.toml"));
    // The headless daemon discovers from its workspace, last value winning.
    let headless = super::config_source(
        &argv(&[
            "daemon",
            "--headless",
            "--workspace",
            "/repo/a",
            "--workspace",
            "/repo/b",
            "--config",
            "/tmp/c.toml",
        ]),
        true,
    );
    assert_eq!(headless.config.as_deref(), Some("/tmp/c.toml"));
    assert_eq!(headless.dir, std::path::PathBuf::from("/repo/b"));
}

#[test]
fn the_hub_resolves_telemetry_config_from_the_account_home() {
    // An explicit environment, not the process one: other tests in this binary
    // repoint `MEDULLA_HOME` process-wide.
    let env: std::collections::HashMap<String, String> =
        [("MEDULLA_HOME".to_string(), "/srv/medulla-home".to_string())].into();
    let hub = super::hub_config_source(&env);
    assert_eq!(hub.config, None);
    assert_eq!(hub.dir, medulla::home::medulla_home(&env));
    // And `hub` routes there however it is invoked.
    let raw = vec![
        "hub".to_string(),
        "--config".into(),
        "/tmp/ignored.toml".into(),
    ];
    assert_eq!(super::config_source(&raw, false).config, None);
}

#[test]
fn a_wrapper_takes_its_config_only_from_an_inherited_config_path() {
    let inherited: std::collections::HashMap<String, String> = [(
        medulla::config::CONFIG_PATH_ENV.to_string(),
        "/tmp/parent.toml".to_string(),
    )]
    .into();
    assert_eq!(
        super::wrapper_config_source(&inherited).config.as_deref(),
        Some("/tmp/parent.toml")
    );
    assert_eq!(
        super::wrapper_config_source(&Default::default()).config,
        None
    );
}
