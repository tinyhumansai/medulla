//! Policy construction tests for host-scoped custom harnesses.

use super::super::policy_from_loaded;
use crate::config::{CustomHarnessConfig, HostSection, LoadedConfig, TuiConfig};
use crate::protocol::HarnessProvider;

/// A minimal, valid custom-harness preset for `host_id`.
fn preset(id: &str, host_id: &str) -> CustomHarnessConfig {
    CustomHarnessConfig {
        id: id.to_string(),
        name: id.to_string(),
        base_harness: HarnessProvider::Claude,
        model: "deepseek/deepseek-chat".to_string(),
        fast_model: None,
        context_window: None,
        host_id: host_id.to_string(),
        default: false,
        api_key_env: "OPENROUTER_API_KEY".to_string(),
        base_url: String::new(),
        codex_overrides: false,
        reasoning_effort: None,
    }
}

/// Writes `presets` as this file's `[[workflows.customHarnesses]]` table and
/// returns a loaded config naming it as a source.
fn loaded_with_presets(
    root: &std::path::Path,
    host_address: &str,
    presets: &[CustomHarnessConfig],
) -> LoadedConfig {
    let path = root.join("medulla.tui.json");
    let rows: Vec<serde_json::Value> = presets
        .iter()
        .map(|preset| serde_json::to_value(preset).unwrap())
        .collect();
    std::fs::write(
        &path,
        serde_json::json!({ "customHarnesses": rows }).to_string(),
    )
    .unwrap();

    let mut config = TuiConfig::default();
    config.host.address = host_address.to_string();

    LoadedConfig {
        config,
        path: path.to_string_lossy().to_string(),
        sources: vec![path.to_string_lossy().to_string()],
    }
}

#[test]
fn presets_for_another_fleet_host_are_not_advertised_or_carried_for_execution() {
    let root = tempfile::tempdir().unwrap();
    let loaded = loaded_with_presets(
        root.path(),
        "this-machine",
        &[
            preset("mine", "this-machine"),
            preset("someone-elses", "other-machine"),
        ],
    );

    let policy = policy_from_loaded(loaded);

    assert_eq!(policy.custom_harnesses, vec!["mine".to_string()]);
    assert_eq!(policy.custom_harness_configs.len(), 1);
    assert_eq!(policy.custom_harness_configs[0].id, "mine");
}

#[test]
fn a_blank_host_address_matches_the_section_default_not_every_preset() {
    let root = tempfile::tempdir().unwrap();
    // The operator never set `[host].address`, so the effective id is the
    // section default — not the blank string a preset might carry.
    let loaded = loaded_with_presets(
        root.path(),
        "",
        &[preset("mine", &HostSection::default().address)],
    );

    let policy = policy_from_loaded(loaded);

    assert_eq!(policy.custom_harnesses, vec!["mine".to_string()]);
}
