//! Focused tests for startup-time local harness availability.

use medulla::config::CustomHarnessConfig;
use medulla::tinyplace::HarnessProvider;

use crate::app_loop::available_primary_presets;

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
