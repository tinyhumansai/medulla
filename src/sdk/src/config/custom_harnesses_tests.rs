//! Tests for named OpenRouter-backed harness presets.

use std::collections::HashMap;

use super::{
    load_custom_harnesses, load_layered_custom_harnesses, CustomHarnessConfig,
    OPENROUTER_ANTHROPIC_URL, OPENROUTER_OPENAI_URL,
};
use crate::protocol::HarnessProvider;

#[test]
fn editor_line_builds_a_claude_openrouter_preset() {
    let preset = CustomHarnessConfig::from_editor_line(
        "deepseek-claude | DeepSeek via Claude | claude | deepseek/deepseek-v4-pro | \
         deepseek/deepseek-v4-flash | this-device",
    )
    .unwrap();
    assert_eq!(preset.id, "deepseek-claude");
    assert_eq!(preset.base_harness, HarnessProvider::Claude);
    assert_eq!(preset.effective_base_url(), OPENROUTER_ANTHROPIC_URL);
    assert_eq!(
        preset.router().api_key_env.as_deref(),
        Some("OPENROUTER_API_KEY")
    );
    assert!(preset
        .harness_env()
        .contains(&("ANTHROPIC_DEFAULT_OPUS_MODEL".into(), preset.model.clone())));
}

#[test]
fn codex_uses_the_openai_endpoint_and_no_claude_tier_environment() {
    let preset = CustomHarnessConfig::from_editor_line(
        "deepseek-codex | DeepSeek via Codex | codex | deepseek/deepseek-v4-pro | | host-2",
    )
    .unwrap();
    assert_eq!(preset.effective_base_url(), OPENROUTER_OPENAI_URL);
    assert!(preset.harness_env().is_empty());
}

#[test]
fn opencode_presets_are_accepted_so_its_traffic_is_attributed_too() {
    // OpenCode was excluded while presets were only an endpoint adapter, since it
    // reaches OpenRouter natively. That native path is the one that bypasses
    // Medulla's attribution proxy, so it is exactly the path that needs a preset.
    let preset = CustomHarnessConfig::from_editor_line(
        "deepseek-oc | DeepSeek via OpenCode | opencode | deepseek/deepseek-v4-pro | | host-3",
    )
    .unwrap();
    assert_eq!(preset.base_harness, HarnessProvider::Opencode);
    assert_eq!(preset.effective_base_url(), OPENROUTER_OPENAI_URL);
    // Its model arrives through the `-m` argument, like Codex's.
    assert!(preset.harness_env().is_empty());
}

#[test]
fn invalid_presets_fail_loudly() {
    let error = CustomHarnessConfig::from_editor_line(
        "bad id! | DeepSeek | claude | deepseek/model | | this-device",
    )
    .unwrap_err();
    assert!(error.contains("id must contain"));
    let error =
        CustomHarnessConfig::from_editor_line("id | name | gemini | model | | host").unwrap_err();
    assert!(error.contains("claude, codex or opencode"));
    let error = CustomHarnessConfig::from_editor_line("id | name | claude | model").unwrap_err();
    assert!(error.contains("expected:"));
}

#[test]
fn key_readiness_checks_only_the_named_environment_value() {
    let preset = CustomHarnessConfig::from_editor_line(
        "deepseek | DeepSeek | codex | deepseek/model | | host",
    )
    .unwrap();
    let mut env = HashMap::new();
    env.insert("OPENROUTER_API_KEY".into(), "   ".into());
    assert!(!preset.key_present(&env));
    env.insert("OPENROUTER_API_KEY".into(), "sk-or-test".into());
    assert!(preset.key_present(&env));
}

#[test]
fn custom_harnesses_load_from_toml_without_reading_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[[customHarnesses]]
id = "deepseek"
name = "DeepSeek via Claude"
baseHarness = "claude"
model = "deepseek/deepseek-v4-pro"
fastModel = "deepseek/deepseek-v4-flash"
hostId = "this-device"
apiKeyEnv = "OPENROUTER_API_KEY"
"#,
    )
    .unwrap();
    let presets = load_custom_harnesses(&path).unwrap();
    assert_eq!(presets.len(), 1);
    assert_eq!(presets[0].id, "deepseek");
}

#[test]
fn layered_loading_preserves_global_presets_across_unrelated_project_settings() {
    let dir = tempfile::tempdir().unwrap();
    let global = dir.path().join("global.toml");
    let project = dir.path().join("project.toml");
    std::fs::write(
        &global,
        r#"
[[customHarnesses]]
id = "deepseek"
name = "DeepSeek"
baseHarness = "codex"
model = "deepseek/model"
hostId = "this-device"
"#,
    )
    .unwrap();
    std::fs::write(&project, "[backend]\nbaseUrl = \"https://example.test\"\n").unwrap();

    let presets = load_layered_custom_harnesses(&[
        global.to_string_lossy().into_owned(),
        project.to_string_lossy().into_owned(),
    ])
    .unwrap();

    assert_eq!(presets.len(), 1);
    assert_eq!(presets[0].id, "deepseek");
}
