//! Pure environment-variable resolution for the memory (tinycortex persona)
//! integration.
//!
//! Every knob the [`MemoryService`](super::MemoryService) reads resolves here as
//! a pure function over an injected `&HashMap<String, String>` (and an injected
//! `home`), so the precedence matrix is unit-testable and the resolver never
//! touches the real process environment or filesystem. Precedence is uniform:
//! the environment override beats the `memory` config section, which beats the
//! built-in default.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::MemoryConfigSection;

/// Default per-run provider spend ceiling (USD), mirroring tinycortex's
/// `PersonaRunBudget` default.
pub const DEFAULT_MAX_COST_USD: f64 = 5.0;

/// The backend catalog's summarization model, used when memory syncs through
/// the tinyhumans backend instead of OpenRouter ("Summarizer V1").
pub const DEFAULT_BACKEND_MODEL: &str = "summarization-v1";

impl MemorySettings {
    /// Attach the backend inference target (base URL + JWT).
    pub fn with_backend(mut self, base_url: impl Into<String>, jwt: impl Into<String>) -> Self {
        self.backend = Some(BackendInference {
            base_url: base_url.into(),
            jwt: jwt.into(),
        });
        self
    }
}

fn non_empty<'a>(env: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    env.get(key).map(|s| s.trim()).filter(|s| !s.is_empty())
}

fn truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Whether the memory surface is enabled. `MEDULLA_MEMORY` (when set to any
/// non-empty value) wins in both directions; otherwise the config `enabled`
/// flag; otherwise ON by default.
pub fn enabled(section: Option<&MemoryConfigSection>, env: &HashMap<String, String>) -> bool {
    if let Some(value) = non_empty(env, "MEDULLA_MEMORY") {
        return truthy(value);
    }
    section.and_then(|s| s.enabled).unwrap_or(true)
}

/// Resolve the workspace root. Order: `TINYCORTEX_WORKSPACE` > config
/// `workspace` > `<medulla_home>/memory`. `medulla_home` is the Medulla home
/// directory ([`crate::home::medulla_home`]).
pub fn workspace(
    section: Option<&MemoryConfigSection>,
    env: &HashMap<String, String>,
    medulla_home: &Path,
) -> PathBuf {
    if let Some(value) = non_empty(env, "TINYCORTEX_WORKSPACE") {
        return PathBuf::from(value);
    }
    if let Some(value) = section
        .and_then(|s| s.workspace.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return PathBuf::from(value);
    }
    medulla_home.join("memory")
}

/// Resolve the pack identity line. Order: `PERSONA_IDENTITY` > config
/// `identity` > empty.
pub fn identity(section: Option<&MemoryConfigSection>, env: &HashMap<String, String>) -> String {
    if let Some(value) = non_empty(env, "PERSONA_IDENTITY") {
        return value.to_string();
    }
    section
        .and_then(|s| s.identity.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("")
        .to_string()
}

fn path_override(
    env: &HashMap<String, String>,
    env_key: &str,
    config_value: Option<&str>,
) -> Option<PathBuf> {
    if let Some(value) = non_empty(env, env_key) {
        return Some(PathBuf::from(value));
    }
    config_value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

/// `PERSONA_CLAUDE_ROOT` > config `claudeRoot` > `None` (tinycortex default).
pub fn claude_root(
    section: Option<&MemoryConfigSection>,
    env: &HashMap<String, String>,
) -> Option<PathBuf> {
    path_override(
        env,
        "PERSONA_CLAUDE_ROOT",
        section.and_then(|s| s.claude_root.as_deref()),
    )
}

/// `PERSONA_CODEX_ROOT` > config `codexRoot` > `None` (tinycortex default).
pub fn codex_root(
    section: Option<&MemoryConfigSection>,
    env: &HashMap<String, String>,
) -> Option<PathBuf> {
    path_override(
        env,
        "PERSONA_CODEX_ROOT",
        section.and_then(|s| s.codex_root.as_deref()),
    )
}

/// Split a `PERSONA_PROJECT_ROOTS` env value into paths (comma-separated,
/// trimmed, empties dropped).
fn split_roots(raw: &str) -> Vec<PathBuf> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Project roots. Order: `PERSONA_PROJECT_ROOTS` (comma-separated) > config
/// `projectRoots` > empty (tinycortex default of `<home>/work` applies).
pub fn project_roots(
    section: Option<&MemoryConfigSection>,
    env: &HashMap<String, String>,
) -> Vec<PathBuf> {
    if let Some(value) = non_empty(env, "PERSONA_PROJECT_ROOTS") {
        return split_roots(value);
    }
    section
        .map(|s| {
            s.project_roots
                .iter()
                .map(|p| p.trim())
                .filter(|p| !p.is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Chat/digest model id. Order: `TINYCORTEX_LLM_MODEL` > config `model` >
/// `None` (tinycortex default).
pub fn llm_model(
    section: Option<&MemoryConfigSection>,
    env: &HashMap<String, String>,
) -> Option<String> {
    if let Some(value) = non_empty(env, "TINYCORTEX_LLM_MODEL") {
        return Some(value.to_string());
    }
    section
        .and_then(|s| s.model.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Per-run spend ceiling. Order: `PERSONA_MAX_COST_USD` (parsed, positive) >
/// config `maxCostUsd` > [`DEFAULT_MAX_COST_USD`].
pub fn max_cost_usd(section: Option<&MemoryConfigSection>, env: &HashMap<String, String>) -> f64 {
    if let Some(value) = non_empty(env, "PERSONA_MAX_COST_USD") {
        if let Ok(parsed) = value.parse::<f64>() {
            if parsed > 0.0 {
                return parsed;
            }
        }
    }
    section
        .and_then(|s| s.max_cost_usd)
        .filter(|v| *v > 0.0)
        .unwrap_or(DEFAULT_MAX_COST_USD)
}

/// The OpenRouter API key, when present and non-empty.
pub fn openrouter_api_key(env: &HashMap<String, String>) -> Option<String> {
    non_empty(env, "OPENROUTER_API_KEY").map(str::to_string)
}

/// Resolve the full [`MemorySettings`] from the optional config section, the
/// environment, and the Medulla home directory.
pub fn resolve(
    section: Option<&MemoryConfigSection>,
    env: &HashMap<String, String>,
    medulla_home: &Path,
) -> MemorySettings {
    MemorySettings {
        enabled: enabled(section, env),
        workspace: workspace(section, env, medulla_home),
        identity: identity(section, env),
        claude_root: claude_root(section, env),
        codex_root: codex_root(section, env),
        project_roots: project_roots(section, env),
        llm_model: llm_model(section, env),
        max_cost_usd: max_cost_usd(section, env),
        openrouter_api_key: openrouter_api_key(env),
        backend: None,
    }
}

/// Resolve [`MemorySettings`] and attach the backend inference target when a
/// backend token is available.
///
/// Extends [`resolve`] by loading stored credentials from the credential store
/// under `medulla_home` and applying the same precedence as
/// [`crate::auth::resolve_backend_token`] (inline config token → `tokenEnv` →
/// matching stored credentials). When a token resolves, summarization syncs
/// through the backend; otherwise the settings are returned unchanged (an
/// explicit `OPENROUTER_API_KEY` still wins inside the service). Reads the
/// credential file from disk.
pub fn resolve_with_backend(
    section: Option<&MemoryConfigSection>,
    backend: &crate::config::BackendConfig,
    env: &HashMap<String, String>,
    medulla_home: &Path,
) -> MemorySettings {
    let settings = resolve(section, env, medulla_home);
    let stored = crate::auth::CredentialStore::at_home(medulla_home).load_or_legacy();
    match crate::auth::resolve_backend_token(env, backend, stored.as_ref()) {
        Some(jwt) => settings.with_backend(backend.base_url.clone(), jwt),
        None => settings,
    }
}

#[cfg(test)]
#[path = "env_tests.rs"]
mod tests;

mod types;
pub use types::BackendInference;
pub use types::MemorySettings;
