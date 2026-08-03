//! Named OpenRouter-backed harness presets.
//!
//! A preset keeps the coding harness and the inference provider separate: the
//! coding CLI remains the agent runtime, while OpenRouter supplies the model and
//! credential. Secrets are referenced by environment-variable name and are never
//! stored in the config document.
//!
//! All three harnesses are accepted. OpenCode was excluded while presets were
//! purely an endpoint adapter — it has native OpenRouter provider configuration
//! and needed no help reaching the API. That native path is precisely the one
//! that bypasses [`crate::inference_proxy`], so an OpenCode run configured
//! outside Medulla spends the operator's credit while crediting OpenCode for the
//! traffic. Routing it through a preset is what brings it under Medulla's
//! attribution.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::RouterConfig;
use crate::protocol::HarnessProvider;

/// The default environment variable containing an OpenRouter API key.
pub const OPENROUTER_API_KEY_ENV: &str = "OPENROUTER_API_KEY";
/// OpenRouter's Anthropic-compatible endpoint used by Claude Code.
pub const OPENROUTER_ANTHROPIC_URL: &str = "https://openrouter.ai/api";
/// OpenRouter's OpenAI-compatible endpoint used by Codex and OpenCode.
pub const OPENROUTER_OPENAI_URL: &str = "https://openrouter.ai/api/v1";

/// The compact editor-line format, and the error shown when a line does not
/// match it. Shared so the TUI's prompt and the parser's rejection cannot drift.
pub const EDITOR_LINE_FORMAT: &str =
    "expected: id | name | claude|codex|opencode | model | fast-model | host-id";

/// One named harness preset that runs an OpenRouter model through a coding CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomHarnessConfig {
    /// Stable fleet-facing identifier.
    pub id: String,
    /// Operator-facing name.
    pub name: String,
    /// Coding harness reused by this preset (`claude` or `codex`).
    pub base_harness: HarnessProvider,
    /// OpenRouter model id used for the main turn.
    pub model: String,
    /// Optional cheaper model used for Claude Code's Sonnet/Haiku tiers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_model: Option<String>,
    /// Optional Claude Code auto-compaction threshold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    /// Fleet host id exposing this preset.
    pub host_id: String,
    /// Whether an untargeted task should use this preset on its host.
    #[serde(default)]
    pub default: bool,
    /// Environment-variable name containing the OpenRouter key.
    #[serde(default = "default_key_env")]
    pub api_key_env: String,
    /// Explicit endpoint override. Empty selects the base-harness default.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub base_url: String,
}

fn default_key_env() -> String {
    OPENROUTER_API_KEY_ENV.to_string()
}

impl CustomHarnessConfig {
    /// Validate and normalize a preset before it is persisted or executed.
    ///
    /// IDs are deliberately shell- and wire-safe because they cross the fleet
    /// protocol. Every built-in harness is accepted — see the module docs for why
    /// OpenCode is no longer excluded.
    pub fn normalize(mut self) -> Result<Self, String> {
        self.id = self.id.trim().to_string();
        self.name = self.name.trim().to_string();
        self.model = self.model.trim().to_string();
        self.host_id = self.host_id.trim().to_string();
        self.api_key_env = self.api_key_env.trim().to_string();
        self.base_url = self.base_url.trim().to_string();
        self.fast_model = self
            .fast_model
            .take()
            .map(|model| model.trim().to_string())
            .filter(|model| !model.is_empty());

        if self.id.is_empty()
            || !self
                .id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        {
            return Err("id must contain only letters, numbers, '.', '-' or '_'".into());
        }
        if self.name.is_empty() {
            return Err("name is required".into());
        }
        if self.model.is_empty() {
            return Err("OpenRouter model id is required".into());
        }
        if self.host_id.is_empty() {
            return Err("host id is required".into());
        }
        if self.api_key_env.is_empty() {
            return Err("API-key environment variable is required".into());
        }
        Ok(self)
    }

    /// Effective OpenRouter endpoint for this preset.
    pub fn effective_base_url(&self) -> &str {
        if !self.base_url.is_empty() {
            &self.base_url
        } else {
            match self.base_harness {
                HarnessProvider::Claude => OPENROUTER_ANTHROPIC_URL,
                HarnessProvider::Codex | HarnessProvider::Opencode | HarnessProvider::Openhuman => {
                    OPENROUTER_OPENAI_URL
                }
            }
        }
    }

    /// Router injection scoped to a run using this preset.
    pub fn router(&self) -> RouterConfig {
        RouterConfig {
            base_url: Some(self.effective_base_url().to_string()),
            api_key_env: Some(self.api_key_env.clone()),
            ..RouterConfig::default()
        }
    }

    /// Non-secret environment overrides needed by the reused coding harness.
    ///
    /// Claude Code has internal model tiers, so mapping all of them is what
    /// keeps sub-agents on OpenRouter too. Codex and OpenCode receive their model
    /// through an existing argument (`-m`) and need no additional variables.
    pub fn harness_env(&self) -> Vec<(String, String)> {
        if self.base_harness != HarnessProvider::Claude {
            return Vec::new();
        }
        let fast = self.fast_model.as_deref().unwrap_or(&self.model);
        let mut env = vec![
            ("ANTHROPIC_DEFAULT_OPUS_MODEL".into(), self.model.clone()),
            ("ANTHROPIC_DEFAULT_SONNET_MODEL".into(), fast.to_string()),
            ("ANTHROPIC_DEFAULT_HAIKU_MODEL".into(), fast.to_string()),
            ("ANTHROPIC_SMALL_FAST_MODEL".into(), fast.to_string()),
        ];
        if let Some(window) = self.context_window {
            env.push(("CLAUDE_CODE_AUTO_COMPACT_WINDOW".into(), window.to_string()));
        }
        env
    }

    /// Whether the referenced OpenRouter key is present and non-blank.
    pub fn key_present(&self, env: &HashMap<String, String>) -> bool {
        env.get(&self.api_key_env)
            .is_some_and(|value| !value.trim().is_empty())
    }

    /// Compact single-line editor representation used by the TUI.
    pub fn editor_line(&self) -> String {
        format!(
            "{} | {} | {} | {} | {} | {}",
            self.id,
            self.name,
            self.base_harness.as_str(),
            self.model,
            self.fast_model.as_deref().unwrap_or(""),
            self.host_id,
        )
    }

    /// Parse the TUI's compact editor line.
    ///
    /// The format is
    /// `id | name | claude|codex|opencode | model | fast-model | host-id`.
    /// Empty fast-model falls back to the main model.
    pub fn from_editor_line(line: &str) -> Result<Self, String> {
        let fields: Vec<&str> = line.split('|').map(str::trim).collect();
        if fields.len() != 6 {
            return Err(EDITOR_LINE_FORMAT.into());
        }
        let base_harness = HarnessProvider::from_wire(fields[2])
            .ok_or_else(|| "base harness must be claude, codex or opencode".to_string())?;
        Self {
            id: fields[0].into(),
            name: fields[1].into(),
            base_harness,
            model: fields[3].into(),
            fast_model: (!fields[4].is_empty()).then(|| fields[4].into()),
            context_window: None,
            host_id: fields[5].into(),
            default: false,
            api_key_env: default_key_env(),
            base_url: String::new(),
        }
        .normalize()
    }
}

/// Read `customHarnesses` from a JSON or TOML config document.
///
/// Missing files and missing sections are empty. Invalid documents and invalid
/// presets return an error naming the source instead of silently dropping fleet
/// capacity the operator expected to exist.
pub fn load_custom_harnesses(path: &Path) -> anyhow::Result<Vec<CustomHarnessConfig>> {
    Ok(load_custom_harness_section(path)?.unwrap_or_default())
}

/// Resolve `customHarnesses` across config sources ordered low to high.
///
/// A higher-precedence source replaces the array only when it declares the
/// section. Unrelated project-local settings therefore do not hide presets
/// inherited from the user-global config.
pub fn load_layered_custom_harnesses(
    sources: &[String],
) -> anyhow::Result<Vec<CustomHarnessConfig>> {
    let mut effective = Vec::new();
    for source in sources {
        if let Some(harnesses) = load_custom_harness_section(Path::new(source))? {
            effective = harnesses;
        }
    }
    Ok(effective)
}

/// Read and validate the section while preserving the distinction between a
/// missing section and a deliberately empty array for layered resolution.
fn load_custom_harness_section(path: &Path) -> anyhow::Result<Option<Vec<CustomHarnessConfig>>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let value: serde_json::Value = if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"))
    {
        let value: toml::Value = toml::from_str(&text)?;
        serde_json::to_value(value)?
    } else {
        serde_json::from_str(&text)?
    };
    let Some(rows) = value.get("customHarnesses").cloned() else {
        return Ok(None);
    };
    let presets: Vec<CustomHarnessConfig> = serde_json::from_value(rows)?;
    let presets = presets
        .into_iter()
        .map(|preset| preset.normalize().map_err(anyhow::Error::msg))
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(Some(presets))
}
