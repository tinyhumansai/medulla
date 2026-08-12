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
fn a_codex_preset_publishes_its_knobs_only_when_it_opts_in() {
    let mut preset = CustomHarnessConfig::from_editor_line(
        "deepseek-codex | DeepSeek via Codex | codex | deepseek/deepseek-v4-pro | | host-2",
    )
    .unwrap();
    // Opting in is what changes which account Codex authenticates as, so an
    // ordinary Codex preset must stay silent — see `crate::codex_overrides`.
    assert!(preset.harness_env().is_empty());

    preset.codex_overrides = true;
    preset.reasoning_effort = Some(" medium ".into());
    preset.context_window = Some(128_000);
    let preset = preset.normalize().unwrap();
    let env: HashMap<String, String> = preset.harness_env().into_iter().collect();
    assert_eq!(env.get(crate::codex_overrides::OVERRIDES_ENV).unwrap(), "1");
    assert_eq!(
        env.get(crate::codex_overrides::EFFORT_ENV).unwrap(),
        "medium"
    );
    assert_eq!(
        env.get(crate::codex_overrides::CONTEXT_WINDOW_ENV).unwrap(),
        "128000"
    );
    assert_eq!(
        env.get(crate::codex_overrides::DISPLAY_NAME_ENV).unwrap(),
        "DeepSeek via Codex"
    );
    // The Claude tier variables belong to Claude Code alone.
    assert!(!env.contains_key("ANTHROPIC_DEFAULT_OPUS_MODEL"));
}

#[test]
fn an_opted_in_preset_without_optional_knobs_publishes_only_activation_and_name() {
    // The previous test only covered the enabled path with both optional
    // values (`reasoningEffort`, `contextWindow`) present. A preset that opts
    // in without naming either must still publish activation and the display
    // name, and nothing else — `codex_overrides::launch_args` supplies its own
    // defaults for the rest.
    let mut preset = CustomHarnessConfig::from_editor_line(
        "deepseek-codex | DeepSeek via Codex | codex | deepseek/deepseek-v4-pro | | host-2",
    )
    .unwrap();
    preset.codex_overrides = true;
    // Blank, not absent: normalize() must still treat it as unset.
    preset.reasoning_effort = Some("   ".into());
    let preset = preset.normalize().unwrap();

    let env: HashMap<String, String> = preset.harness_env().into_iter().collect();
    assert_eq!(env.get(crate::codex_overrides::OVERRIDES_ENV).unwrap(), "1");
    assert_eq!(
        env.get(crate::codex_overrides::DISPLAY_NAME_ENV).unwrap(),
        "DeepSeek via Codex"
    );
    assert!(!env.contains_key(crate::codex_overrides::EFFORT_ENV));
    assert!(!env.contains_key(crate::codex_overrides::CONTEXT_WINDOW_ENV));
    assert_eq!(env.len(), 2, "{env:?}");
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
    assert!(error.contains("claude, codex, opencode or openhuman"));
    let error = CustomHarnessConfig::from_editor_line("id | name | claude | model").unwrap_err();
    assert!(error.contains("expected:"));
}

#[test]
fn openhuman_presets_are_accepted_so_the_embedded_core_can_be_given_a_model() {
    // The refusal predated OpenHuman being dispatchable at all. Now that a
    // workflow node can name it, the only thing the refusal blocked was naming
    // the model that turn runs on — which is what a preset is for.
    let preset = CustomHarnessConfig::from_editor_line(
        "deepseek-oh | DeepSeek via OpenHuman | openhuman | deepseek/deepseek-v4-pro | | host-4",
    )
    .unwrap();
    assert_eq!(preset.base_harness, HarnessProvider::Openhuman);
    // Nothing rides down in the environment: there is no child process to hand
    // one to. The model reaches the turn as `RunTaskOptions::model`.
    assert!(preset.harness_env().is_empty());
    assert_eq!(preset.model, "deepseek/deepseek-v4-pro");
}

#[test]
fn an_openhuman_preset_needs_no_openrouter_key_to_be_usable() {
    // The embedded core authenticates its own turns with the operator's
    // account, so gating the preset on an OpenRouter key would hide a working
    // preset on every machine that never set one.
    let preset = CustomHarnessConfig::from_editor_line(
        "oh | OpenHuman | openhuman | some/model | | this-device",
    )
    .unwrap();
    assert!(preset.key_present(&HashMap::new()));
}

#[test]
fn persisted_openhuman_preset_loads_with_its_model() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[[customHarnesses]]
id = "native"
name = "Native OpenHuman"
baseHarness = "openhuman"
model = "deepseek/deepseek-v4-pro"
hostId = "this-device"
"#,
    )
    .unwrap();

    let presets = load_custom_harnesses(&path).unwrap();
    assert_eq!(presets.len(), 1);
    assert_eq!(presets[0].base_harness, HarnessProvider::Openhuman);
    assert_eq!(presets[0].model, "deepseek/deepseek-v4-pro");
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

#[test]
fn runnability_follows_the_detected_clis_except_for_the_embedded_core() {
    let claude = CustomHarnessConfig::from_editor_line(
        "ds | DeepSeek | claude | deepseek/model | | this-device",
    )
    .unwrap();
    assert!(claude.runnable_on(&[HarnessProvider::Claude]));
    assert!(!claude.runnable_on(&[HarnessProvider::Codex]));

    // The embedded core has no binary to detect, so a host that found no
    // coding CLI at all still runs it — the same rule `select_provider`
    // applies to a bare `openhuman` request.
    let openhuman =
        CustomHarnessConfig::from_editor_line("oh | OpenHuman | openhuman | some/model | | host")
            .unwrap();
    assert!(openhuman.runnable_on(&[]));
}

#[test]
fn an_upstream_provider_pin_is_normalized_and_reaches_the_router() {
    let preset: CustomHarnessConfig = serde_json::from_value(serde_json::json!({
        "id": "glm",
        "name": "GLM 5.2 via Claude",
        "baseHarness": "claude",
        "model": "z-ai/glm-5.2",
        "hostId": "this-device",
        // As an operator might copy them off OpenRouter's dashboard, which
        // presents slugs capitalized and padded. `streamlake` recurs after
        // `novita`, so a naive adjacent-only dedup would leave the repeat.
        "providerOnly": ["  StreamLake ", "Novita", "", "streamlake"],
    }))
    .unwrap();
    let preset = preset.normalize().unwrap();

    assert_eq!(preset.provider_only, vec!["streamlake", "novita"]);
    // Order is the operator's preference order and must survive.
    assert_eq!(preset.router().provider_only, vec!["streamlake", "novita"]);
}

#[test]
fn an_unpinned_preset_leaves_provider_choice_to_openrouter() {
    let preset =
        CustomHarnessConfig::from_editor_line("glm | GLM | claude | z-ai/glm-5.2 | | this-device")
            .unwrap();
    assert!(preset.provider_only.is_empty());
    assert!(preset.router().provider_only.is_empty());
}

#[test]
fn a_pin_round_trips_through_a_config_document() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("medulla.toml");
    std::fs::write(
        &path,
        r#"
[[customHarnesses]]
id = "glm"
name = "GLM 5.2 via Claude"
baseHarness = "claude"
model = "z-ai/glm-5.2"
hostId = "this-device"
providerOnly = ["streamlake"]
"#,
    )
    .unwrap();

    let presets = super::load_custom_harnesses(&path).unwrap();
    assert_eq!(presets.len(), 1);
    assert_eq!(presets[0].provider_only, vec!["streamlake"]);
}
