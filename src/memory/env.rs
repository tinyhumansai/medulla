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

/// The resolved, medulla-owned memory settings (no vendor types leak here).
#[derive(Debug, Clone, PartialEq)]
pub struct MemorySettings {
    /// Whether the memory surface is active at all.
    pub enabled: bool,
    /// Workspace root for the SQLite chunk store, facet trees, and `persona/`.
    pub workspace: PathBuf,
    /// Identity line for the compiled pack header (email / name).
    pub identity: String,
    /// Claude Code transcript root override (`None` = tinycortex default).
    pub claude_root: Option<PathBuf>,
    /// Codex rollout root override (`None` = tinycortex default).
    pub codex_root: Option<PathBuf>,
    /// Project roots walked for instruction files + git history. Empty = default.
    pub project_roots: Vec<PathBuf>,
    /// Chat/digest model id override (`None` = tinycortex default).
    pub llm_model: Option<String>,
    /// Per-run provider spend ceiling, USD.
    pub max_cost_usd: f64,
    /// The OpenRouter API key, when present (ingest requires it).
    pub openrouter_api_key: Option<String>,
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
/// flag; otherwise off.
pub fn enabled(section: Option<&MemoryConfigSection>, env: &HashMap<String, String>) -> bool {
    if let Some(value) = non_empty(env, "MEDULLA_MEMORY") {
        return truthy(value);
    }
    section.and_then(|s| s.enabled).unwrap_or(false)
}

/// Resolve the workspace root. Order: `TINYCORTEX_WORKSPACE` > config
/// `workspace` > `<medulla_home>/memory`. `medulla_home` is the Medulla home
/// directory (see [`crate::memory::default_medulla_home`]).
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn section() -> MemoryConfigSection {
        MemoryConfigSection {
            enabled: Some(true),
            workspace: Some("/cfg/ws".into()),
            identity: Some("cfg@example.com".into()),
            claude_root: Some("/cfg/claude".into()),
            codex_root: Some("/cfg/codex".into()),
            project_roots: vec!["/cfg/a".into(), "/cfg/b".into()],
            model: Some("cfg/model".into()),
            max_cost_usd: Some(2.5),
        }
    }

    #[test]
    fn enabled_env_beats_config_both_ways() {
        // Config on, env off → off.
        assert!(!enabled(Some(&section()), &env(&[("MEDULLA_MEMORY", "0")])));
        // Config absent, env on → on.
        assert!(enabled(None, &env(&[("MEDULLA_MEMORY", "true")])));
        // Config on, no env → on.
        assert!(enabled(Some(&section()), &env(&[])));
        // Nothing → off.
        assert!(!enabled(None, &env(&[])));
    }

    #[test]
    fn workspace_precedence() {
        let home = PathBuf::from("/home/u/.medulla");
        // Default is `<medulla_home>/memory`.
        assert_eq!(
            workspace(None, &env(&[]), &home),
            PathBuf::from("/home/u/.medulla/memory")
        );
        // Config beats default.
        assert_eq!(
            workspace(Some(&section()), &env(&[]), &home),
            PathBuf::from("/cfg/ws")
        );
        // Env beats config.
        assert_eq!(
            workspace(
                Some(&section()),
                &env(&[("TINYCORTEX_WORKSPACE", "/env/ws")]),
                &home
            ),
            PathBuf::from("/env/ws")
        );
    }

    #[test]
    fn identity_and_model_precedence() {
        assert_eq!(identity(None, &env(&[])), "");
        assert_eq!(identity(Some(&section()), &env(&[])), "cfg@example.com");
        assert_eq!(
            identity(Some(&section()), &env(&[("PERSONA_IDENTITY", "env@x")])),
            "env@x"
        );
        assert_eq!(llm_model(None, &env(&[])), None);
        assert_eq!(
            llm_model(Some(&section()), &env(&[])).as_deref(),
            Some("cfg/model")
        );
        assert_eq!(
            llm_model(Some(&section()), &env(&[("TINYCORTEX_LLM_MODEL", "env/m")])).as_deref(),
            Some("env/m")
        );
    }

    #[test]
    fn roots_precedence_and_split() {
        assert!(project_roots(None, &env(&[])).is_empty());
        assert_eq!(
            project_roots(Some(&section()), &env(&[])),
            vec![PathBuf::from("/cfg/a"), PathBuf::from("/cfg/b")]
        );
        assert_eq!(
            project_roots(
                Some(&section()),
                &env(&[("PERSONA_PROJECT_ROOTS", " /env/x , /env/y ,")])
            ),
            vec![PathBuf::from("/env/x"), PathBuf::from("/env/y")]
        );
        assert_eq!(
            claude_root(Some(&section()), &env(&[])),
            Some(PathBuf::from("/cfg/claude"))
        );
        assert_eq!(
            codex_root(None, &env(&[("PERSONA_CODEX_ROOT", "/env/codex")])),
            Some(PathBuf::from("/env/codex"))
        );
        assert_eq!(claude_root(None, &env(&[])), None);
    }

    #[test]
    fn max_cost_precedence_and_guards() {
        assert_eq!(max_cost_usd(None, &env(&[])), DEFAULT_MAX_COST_USD);
        assert_eq!(max_cost_usd(Some(&section()), &env(&[])), 2.5);
        assert_eq!(
            max_cost_usd(Some(&section()), &env(&[("PERSONA_MAX_COST_USD", "9.0")])),
            9.0
        );
        // Non-positive / garbage env falls back to config.
        for bad in ["0", "-1", "abc"] {
            assert_eq!(
                max_cost_usd(Some(&section()), &env(&[("PERSONA_MAX_COST_USD", bad)])),
                2.5
            );
        }
    }

    #[test]
    fn api_key_presence() {
        assert_eq!(openrouter_api_key(&env(&[])), None);
        assert_eq!(
            openrouter_api_key(&env(&[("OPENROUTER_API_KEY", "  ")])),
            None
        );
        assert_eq!(
            openrouter_api_key(&env(&[("OPENROUTER_API_KEY", "sk-x")])).as_deref(),
            Some("sk-x")
        );
    }

    #[test]
    fn resolve_composes_all_knobs() {
        let home = PathBuf::from("/home/u/.medulla");
        let s = resolve(
            Some(&section()),
            &env(&[("OPENROUTER_API_KEY", "sk-x"), ("MEDULLA_MEMORY", "1")]),
            &home,
        );
        assert!(s.enabled);
        assert_eq!(s.workspace, PathBuf::from("/cfg/ws"));
        assert_eq!(s.identity, "cfg@example.com");
        assert_eq!(s.openrouter_api_key.as_deref(), Some("sk-x"));
        assert_eq!(s.max_cost_usd, 2.5);
        assert_eq!(s.project_roots.len(), 2);
    }
}
