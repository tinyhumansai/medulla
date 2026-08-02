//! Layered config discovery, parsing, and merge — the [`load_config`] entry
//! point.
//!
//! Files are discovered (or taken verbatim from an explicit `--config`), read as
//! JSON or TOML by extension, deep-merged low → high precedence, deserialized
//! into a [`TuiConfig`], and then finished with environment- and home-derived
//! values that serde defaults cannot compute.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::home::medulla_home;

use super::types::{LoadedConfig, TinyplaceConfig, TuiConfig};
use super::urls::{resolve_backend_base_url, resolve_tinyplace_base_url};

/// The `[tinyplace]` section to use when the config file has none.
///
/// [`load_config`] env-resolves `base_url` and the identity dir only for a
/// section that is actually *present*. A caller that synthesizes its own with
/// [`TinyplaceConfig::default`] therefore gets the **prod** relay even under
/// `MEDULLA_STAGING=1`, because that field's serde default is a constant and
/// constants cannot read the environment.
///
/// That divergence is not cosmetic: a worker pointed at one relay and an
/// orchestrator pointed at another both start cleanly, publish keys, and report
/// healthy — they simply never see each other, because a contact request
/// delivered to one relay does not exist on the other. Use this instead of
/// `Default::default()` anywhere a missing section is filled in, so running
/// without a `[tinyplace]` section cannot strand a peer on a different relay
/// than the rest of the deployment.
pub fn default_tinyplace_config(env: &HashMap<String, String>) -> TinyplaceConfig {
    TinyplaceConfig {
        base_url: resolve_tinyplace_base_url(env, None),
        identity_dir: medulla_home(env)
            .join("tinyplace")
            .to_string_lossy()
            .into_owned(),
        ..TinyplaceConfig::default()
    }
}

/// Turn a path into a canonical (or best-effort) absolute-ish display string.
fn display_path(path: &Path) -> String {
    std::fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

/// Read one config file into a JSON `Value`, choosing the parser by extension
/// (`.toml` → TOML, everything else → JSON). A missing file yields `None`; a
/// present-but-invalid file is an error.
fn read_config_value(path: &Path) -> anyhow::Result<Option<Value>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(anyhow::anyhow!(
                "Cannot read config at {}: {err}",
                display_path(path)
            ))
        }
    };
    let is_toml = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("toml"))
        .unwrap_or(false);
    let value = if is_toml {
        let parsed: toml::Value = toml::from_str(&text)
            .map_err(|err| anyhow::anyhow!("Invalid TOML in {}: {err}", display_path(path)))?;
        serde_json::to_value(parsed)
            .map_err(|err| anyhow::anyhow!("Invalid TOML in {}: {err}", display_path(path)))?
    } else {
        serde_json::from_str(&text)
            .map_err(|err| anyhow::anyhow!("Invalid JSON in {}: {err}", display_path(path)))?
    };
    Ok(Some(value))
}

/// Recursively merge `overlay` into `base`: tables are merged key-by-key (so a
/// project-local file can override just `backend.baseUrl`); any non-table value
/// replaces whatever was there.
pub(super) fn merge_value(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(overlay_map)) => {
            for (key, value) in overlay_map {
                match base_map.get_mut(&key) {
                    Some(existing) => merge_value(existing, value),
                    None => {
                        base_map.insert(key, value);
                    }
                }
            }
        }
        (slot, value) => *slot = value,
    }
}

/// Resolve the ordered list of config files to merge (low → high precedence).
///
/// An explicit `--config` path bypasses discovery and is the sole file. Without
/// one, discovery layers the user-global `<home>/config.toml` under the
/// project-local file (`./.medulla/config.toml`, else `./medulla.toml`).
fn config_file_layers(explicit_config: Option<&str>, home: &Path, cwd: &Path) -> Vec<PathBuf> {
    if let Some(path) = explicit_config {
        return vec![PathBuf::from(path)];
    }
    let mut layers = vec![home.join("config.toml")];
    let project_local = cwd.join(".medulla").join("config.toml");
    if project_local.exists() {
        layers.push(project_local);
    } else {
        layers.push(cwd.join("medulla.toml"));
    }
    layers
}

/// Load and parse the layered TUI config, resolving endpoint base URLs and
/// home-derived paths against `env`.
///
/// Precedence (highest wins): env vars > project-local config
/// (`./.medulla/config.toml` or `./medulla.toml`) > user-global
/// `<home>/config.toml` > built-in defaults. An explicit `--config` path bypasses
/// file discovery (JSON or TOML by extension) but env still overrides on top.
///
/// Base-URL precedence is applied here rather than via serde defaults (which
/// cannot see the environment): backend `MEDULLA_API_URL` > config `baseUrl` >
/// staging/prod default; tiny.place config `baseUrl` > staging/prod default.
/// `stateDir` and `tinyplace.identityDir` default to `<home>/state` and
/// `<home>/tinyplace` when not explicitly configured; `MEDULLA_STATE_DIR`
/// overrides `stateDir`.
///
/// `[fleet].agentTemplates` has one extra layer beneath it: when no config file
/// names it, the `.medulla/agents` TOML store (see [`crate::agents`]) replaces
/// the built-in catalog if it holds anything at all.
pub fn load_config(
    explicit_config: Option<&str>,
    env: &HashMap<String, String>,
    cwd: &Path,
) -> anyhow::Result<LoadedConfig> {
    let home = medulla_home(env);
    let layers = config_file_layers(explicit_config, &home, cwd);

    // Merge every present file (low → high) into one JSON table. The merged
    // value doubles as the "raw" used to tell an explicitly-set field from a
    // serde-defaulted one, so an explicit config value beats a default.
    let mut merged = Value::Object(serde_json::Map::new());
    let mut sources: Vec<String> = Vec::new();
    for layer in &layers {
        if let Some(value) = read_config_value(layer)? {
            merge_value(&mut merged, value);
            sources.push(display_path(layer));
        }
    }

    let has_content = merged.as_object().map(|m| !m.is_empty()).unwrap_or(false);
    let mut config: TuiConfig = if has_content {
        serde_json::from_value(merged.clone())
            .map_err(|err| anyhow::anyhow!("Invalid config: {err}"))?
    } else {
        TuiConfig::default()
    };

    let backend_url = merged
        .get("backend")
        .and_then(|b| b.get("baseUrl"))
        .and_then(|v| v.as_str());
    config.backend.base_url = resolve_backend_base_url(env, backend_url);

    if let Some(tp) = config.tinyplace.as_mut() {
        let tp_raw = merged.get("tinyplace");
        let tp_url = tp_raw
            .and_then(|t| t.get("baseUrl"))
            .and_then(|v| v.as_str());
        tp.base_url = resolve_tinyplace_base_url(env, tp_url);
        // Home-derived identity dir unless the file set one explicitly.
        let id_explicit = tp_raw
            .and_then(|t| t.get("identityDir"))
            .map(|v| !v.is_null())
            .unwrap_or(false);
        if !id_explicit {
            tp.identity_dir = home.join("tinyplace").to_string_lossy().into_owned();
        }
    }

    // stateDir: MEDULLA_STATE_DIR env override > explicit config > <home>/state.
    let state_explicit = merged
        .get("stateDir")
        .map(|v| !v.is_null())
        .unwrap_or(false);
    if let Some(dir) = env
        .get("MEDULLA_STATE_DIR")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        config.state_dir = dir.to_string();
    } else if !state_explicit {
        config.state_dir = home.join("state").to_string_lossy().into_owned();
    }

    // The `.medulla/agents` store supersedes the built-in catalog, but never an
    // explicit `[fleet].agentTemplates` — a config file that names templates
    // (including an empty list, which opts out) has already said what it wants.
    let templates_explicit = merged
        .get("fleet")
        .and_then(|f| f.get("agentTemplates"))
        .map(|v| !v.is_null())
        .unwrap_or(false);
    if !templates_explicit {
        let store = crate::agents::load_templates(&crate::agents::template_dirs(env, cwd));
        if !store.is_empty() {
            config.fleet.agent_templates = store.templates;
        }
    }

    let path = if let Some(explicit) = explicit_config {
        display_path(Path::new(explicit))
    } else {
        sources
            .last()
            .cloned()
            .unwrap_or_else(|| "(built-in defaults)".to_string())
    };

    Ok(LoadedConfig {
        config,
        path,
        sources,
    })
}

/// The environment variable carrying the active `--config` path to a
/// subprocess this process spawns.
///
/// A subprocess (the `medulla workflow mcp` tool server, an ACP harness) does
/// not receive the parent's parsed CLI flags — only its environment, which
/// [`std::process::Command`] inherits by default. Without this, every such
/// subprocess re-ran config *discovery* from its own `cwd`, silently ignoring
/// whatever `--config` the operator launched the parent with: a custom config
/// with `allowCode = false` could still have scripts executed by a session that
/// re-discovered a more permissive default config. The top-level `--config`
/// parse sets this once at startup (see `medulla-tui`'s `run_tui`); every
/// [`load_config`] call made on behalf of a spawned or served subprocess reads
/// it back with [`explicit_config_from_env`] instead of passing `None`.
pub const CONFIG_PATH_ENV: &str = "MEDULLA_CONFIG_PATH";

/// The `--config` path to use for a subprocess, if the parent process recorded
/// one via [`CONFIG_PATH_ENV`].
///
/// Centralised so every subprocess call site (the MCP tool server, the ACP
/// harness's advertised MCP servers) resolves the same way rather than each
/// re-implementing "read this one env var, else `None`".
pub fn explicit_config_from_env(env: &HashMap<String, String>) -> Option<&str> {
    env.get(CONFIG_PATH_ENV).map(String::as_str)
}
