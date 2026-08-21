//! Named OpenRouter-backed harness presets.
//!
//! A preset keeps the coding harness and the inference provider separate: the
//! coding CLI remains the agent runtime, while OpenRouter supplies the model and
//! credential. Secrets are referenced by environment-variable name and are never
//! stored in the config document.
//!
//! Every harness is accepted. OpenCode was excluded while presets were
//! purely an endpoint adapter — it has native OpenRouter provider configuration
//! and needed no help reaching the API. That native path is precisely the one
//! that bypasses [`crate::inference_proxy`], so an OpenCode run configured
//! outside Medulla spends the operator's credit while crediting OpenCode for the
//! traffic. Routing it through a preset is what brings it under Medulla's
//! attribution.
//!
//! # OpenHuman presets
//!
//! OpenHuman was refused outright while it was not dispatchable at all. It is
//! now — a workflow node may name the harness id `openhuman` and the turn runs
//! in this process on the embedded core — so the refusal only stopped an
//! operator naming the model that turn should use, which is the one thing a
//! preset is for. A preset with `baseHarness = "openhuman"` therefore behaves
//! like any other: its `model` reaches the turn (see
//! [`crate::daemon::providers::openhuman::effective_model`] for where it sits
//! among the other routes).
//!
//! `baseUrl` and `apiKeyEnv` are live here too, though they arrive by a
//! different road. For a spawned CLI they are layered into the child's
//! environment; an OpenHuman turn has no child, so Medulla resolves the key and
//! passes the endpoint and the credential to the core as a **per-call** route
//! that the core applies to that turn alone and never persists. An OpenRouter
//! endpoint is exchanged for a loopback mount and a machine-local token at the
//! attribution proxy first; any other endpoint is handed over as spelled. See
//! [`crate::daemon::providers::openhuman::embedded_route`].
//!
//! A preset that leaves them at their defaults still works and still needs no
//! OpenRouter key: with no key exported under `apiKeyEnv` the turn runs on the
//! account's own OpenHuman configuration, which is what an operator who
//! configured only a model is asking for.

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
    "expected: id | name | claude|codex|opencode|openhuman | model | fast-model | host-id";

/// One named harness preset that runs an OpenRouter model through a coding CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomHarnessConfig {
    /// Stable fleet-facing identifier.
    pub id: String,
    /// Operator-facing name.
    pub name: String,
    /// Harness this preset runs on: a coding CLI (`claude`, `codex`,
    /// `opencode`) or the embedded core (`openhuman`).
    pub base_harness: HarnessProvider,
    /// OpenRouter model id used for the main turn.
    pub model: String,
    /// Optional cheaper model used for Claude Code's Sonnet/Haiku tiers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_model: Option<String>,
    /// Optional context window, in tokens: Claude Code's auto-compaction
    /// threshold, and the window a `codexOverrides` preset declares to Codex.
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
    /// Whether a Codex preset spawns with Medulla's Codex config overrides.
    ///
    /// Off by default because it changes which account a Codex run authenticates
    /// as. See [`crate::codex_overrides`] for what it injects and why an
    /// endpoint override alone does not get a non-OpenAI model answering.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub codex_overrides: bool,
    /// Reasoning effort declared to Codex when `codexOverrides` is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    /// OpenRouter serving providers this preset is restricted to, by slug
    /// (`streamlake`, `novita`, …). Empty leaves the choice to OpenRouter.
    ///
    /// The same model is served by many providers at prices that differ by more
    /// than an order of magnitude, and OpenRouter's own default weighs price
    /// against uptime and throughput rather than pinning one. Naming the
    /// provider here is the only way to *state* the choice: the preference
    /// travels in the request body, so neither the model id nor the endpoint
    /// override can carry it. [`crate::inference_proxy`] applies it.
    ///
    /// Inert for an `openhuman` preset, which has no proxied request to amend.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_only: Vec<String>,
}

fn default_key_env() -> String {
    OPENROUTER_API_KEY_ENV.to_string()
}

impl CustomHarnessConfig {
    /// Validate and normalize a preset before it is persisted or executed.
    ///
    /// IDs are deliberately shell- and wire-safe because they cross the fleet
    /// protocol. Every built-in harness is accepted — see the module docs for
    /// why neither OpenCode nor OpenHuman is excluded any longer.
    pub fn normalize(mut self) -> Result<Self, String> {
        self.id = self.id.trim().to_string();
        self.name = self.name.trim().to_string();
        self.model = self.model.trim().to_string();
        self.host_id = self.host_id.trim().to_string();
        self.api_key_env = self.api_key_env.trim().to_string();
        self.base_url = self.base_url.trim().to_string();
        self.reasoning_effort = self
            .reasoning_effort
            .take()
            .map(|effort| effort.trim().to_string())
            .filter(|effort| !effort.is_empty());
        self.fast_model = self
            .fast_model
            .take()
            .map(|model| model.trim().to_string())
            .filter(|model| !model.is_empty());
        // Slugs are lowercased because OpenRouter matches them case-sensitively
        // while its own documentation and dashboard present them capitalized
        // ("StreamLake"). A pin that silently matched nothing would read as
        // OpenRouter ignoring the setting rather than as a typo here. The first
        // occurrence of each slug is kept, across the whole list rather than
        // just adjacent entries: `dedup()` alone would leave a repeat that
        // appears after a different slug.
        let mut seen = std::collections::HashSet::new();
        self.provider_only = std::mem::take(&mut self.provider_only)
            .into_iter()
            .map(|slug| slug.trim().to_ascii_lowercase())
            .filter(|slug| !slug.is_empty())
            .filter(|slug| seen.insert(slug.clone()))
            .collect();

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
        // A preset is a model behind a CLI, and a shell has neither. Refused
        // here rather than shrugged off later so the picker never offers a
        // "custom harness" that cannot route anywhere.
        if self.base_harness == HarnessProvider::Shell {
            return Err("a shell cannot be used as a custom harness".into());
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
                // Shell is rejected by `normalize`, so no preset reaches here
                // with one; it shares the OpenAI-shaped default rather than
                // being a fourth case nobody can produce.
                HarnessProvider::Codex
                | HarnessProvider::Opencode
                | HarnessProvider::Openhuman
                | HarnessProvider::Shell => OPENROUTER_OPENAI_URL,
            }
        }
    }

    /// Router injection scoped to a run using this preset.
    pub fn router(&self) -> RouterConfig {
        RouterConfig {
            base_url: Some(self.effective_base_url().to_string()),
            api_key_env: Some(self.api_key_env.clone()),
            provider_only: self.provider_only.clone(),
            ..RouterConfig::default()
        }
    }

    /// Non-secret environment overrides needed by the reused coding harness.
    ///
    /// Claude Code has internal model tiers, so mapping all of them is what
    /// keeps sub-agents on OpenRouter too. OpenCode receives its model through an
    /// existing argument (`-m`) and needs no additional variables, and OpenHuman
    /// has no child process to hand an environment to at all — its model rides
    /// down as
    /// [`RunTaskOptions::model`](crate::daemon::providers::RunTaskOptions::model)
    /// and becomes the core call's `model_override`.
    ///
    /// Codex takes its model the same way, but a preset with `codexOverrides`
    /// also carries its Codex knobs here, because the environment is what every
    /// spawn seam already hands the child unchanged — see
    /// [`crate::codex_overrides`], which reads them back at the seam.
    pub fn harness_env(&self) -> Vec<(String, String)> {
        if self.base_harness == HarnessProvider::Codex {
            return self.codex_env();
        }
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

    /// The Codex knobs this preset publishes into a run's environment.
    ///
    /// Empty unless the preset opted in: an ordinary Codex preset keeps Codex's
    /// own defaults, and this must not quietly change which account it uses.
    fn codex_env(&self) -> Vec<(String, String)> {
        if !self.codex_overrides {
            return Vec::new();
        }
        let mut env = vec![
            (crate::codex_overrides::OVERRIDES_ENV.into(), "1".into()),
            (
                crate::codex_overrides::DISPLAY_NAME_ENV.into(),
                self.name.clone(),
            ),
        ];
        if let Some(effort) = self
            .reasoning_effort
            .as_deref()
            .map(str::trim)
            .filter(|effort| !effort.is_empty())
        {
            env.push((
                crate::codex_overrides::EFFORT_ENV.into(),
                effort.to_string(),
            ));
        }
        if let Some(window) = self.context_window {
            env.push((
                crate::codex_overrides::CONTEXT_WINDOW_ENV.into(),
                window.to_string(),
            ));
        }
        env
    }

    /// Whether the referenced OpenRouter key is present and non-blank.
    ///
    /// Always true for an OpenHuman preset. The key gates a preset because a
    /// spawned CLI cannot reach the routed endpoint without it, and the
    /// embedded core reaches no such endpoint: it authenticates its own turns
    /// with the account the operator is already signed in as. Gating it on an
    /// OpenRouter key would hide a perfectly usable preset on any machine that
    /// never set one.
    pub fn key_present(&self, env: &HashMap<String, String>) -> bool {
        if self.base_harness == HarnessProvider::Openhuman {
            return true;
        }
        env.get(&self.api_key_env)
            .is_some_and(|value| !value.trim().is_empty())
    }

    /// Whether a host offering `providers` can actually run this preset.
    ///
    /// `providers` is the list of coding CLIs found on `PATH`, and the embedded
    /// core is never on it — it has no binary to detect. So an OpenHuman preset
    /// is runnable wherever Medulla itself is running, matching the rule
    /// `select_provider` already applies to a bare `openhuman` request. Without
    /// this a configured OpenHuman preset would be filtered out of the fleet
    /// advert and the TUI's harness list, and read as broken rather than as
    /// unadvertised.
    pub fn runnable_on(&self, providers: &[HarnessProvider]) -> bool {
        self.base_harness == HarnessProvider::Openhuman || providers.contains(&self.base_harness)
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
    /// `id | name | claude|codex|opencode|openhuman | model | fast-model | host-id`.
    /// Empty fast-model falls back to the main model.
    pub fn from_editor_line(line: &str) -> Result<Self, String> {
        let fields: Vec<&str> = line.split('|').map(str::trim).collect();
        if fields.len() != 6 {
            return Err(EDITOR_LINE_FORMAT.into());
        }
        let base_harness = HarnessProvider::from_wire(fields[2]).ok_or_else(|| {
            "base harness must be claude, codex, opencode or openhuman".to_string()
        })?;
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
            // The compact line has no room for the Codex knobs; a preset that
            // wants them is written in the config file, which is also where the
            // account-changing decision belongs. An upstream-provider pin is
            // similarly absent from the line. Editing an existing preset through
            // the TUI preserves both kinds of field for that reason — the editor
            // save flow restores these alongside the other file-only fields.
            codex_overrides: false,
            reasoning_effort: None,
            provider_only: Vec::new(),
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
