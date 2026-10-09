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
    // `run` takes only `--config <path>`; `--config=…` is instruction text.
    assert_eq!(
        super::explicit_config_path(&argv(&["run", "set", "--config=/tmp/other.toml"])),
        None
    );
    assert_eq!(
        super::explicit_config_path(&argv(&["run", "--config", "/tmp/a.toml", "go"])).as_deref(),
        Some("/tmp/a.toml")
    );
    // A wrapper's flags belong to the child harness.
    assert_eq!(
        super::explicit_config_path(&argv(&["codex", "--config", "model=o3"])),
        None
    );
    assert_eq!(
        super::explicit_config_path(&argv(&["--config", "/tmp/t.toml"])).as_deref(),
        Some("/tmp/t.toml")
    );
    // Only `mcp` takes the joined spelling; elsewhere it is not a config path.
    assert_eq!(
        super::explicit_config_path(&argv(&["workflow", "run", "x", "--config=/tmp/b.toml"])),
        None
    );
    assert_eq!(
        super::explicit_config_path(&argv(&["workflow", "run", "x", "--config", "/tmp/b.toml"]))
            .as_deref(),
        Some("/tmp/b.toml")
    );
    #[cfg(feature = "workflows")]
    assert_eq!(
        super::explicit_config_path(&argv(&["mcp", "--config=/tmp/m.toml"])).as_deref(),
        Some("/tmp/m.toml")
    );
}
