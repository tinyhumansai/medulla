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
    // Only `mcp` takes the joined spelling; elsewhere it is not a config path.
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
