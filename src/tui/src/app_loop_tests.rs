//! Focused tests for startup-time local harness availability.

use medulla::config::CustomHarnessConfig;
use medulla::protocol::HarnessProvider;

use crate::app_loop::{available_primary_presets, validate_explicit_config};

fn preset(id: &str, base_harness: HarnessProvider, host_id: &str) -> CustomHarnessConfig {
    CustomHarnessConfig {
        id: id.into(),
        name: id.into(),
        base_harness,
        model: "openrouter/model".into(),
        fast_model: None,
        context_window: None,
        host_id: host_id.into(),
        default: false,
        api_key_env: "OPENROUTER_API_KEY".into(),
        base_url: String::new(),
        codex_overrides: false,
        reasoning_effort: None,
    }
}

#[test]
fn primary_presets_require_their_base_cli_to_be_available() {
    let presets = vec![
        preset("available", HarnessProvider::Codex, "local"),
        preset("missing-cli", HarnessProvider::Claude, "local"),
        preset("other-host", HarnessProvider::Codex, "remote"),
    ];

    let available = available_primary_presets(&presets, "local", &[HarnessProvider::Codex]);

    assert_eq!(
        available
            .iter()
            .map(|preset| preset.id.as_str())
            .collect::<Vec<_>>(),
        vec!["available"]
    );
}

#[test]
fn a_missing_explicit_tui_config_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.toml");

    let error = validate_explicit_config(missing.to_str()).unwrap_err();

    assert!(error.to_string().contains("does not exist"));
    assert!(validate_explicit_config(None).is_ok());
}
