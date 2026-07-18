//! `medulla.tui.json`-compatible config — the subset the TUI reads, plus a
//! `backend` section for the HTTP runtime. Permissive: missing fields take
//! defaults, unknown fields are ignored.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::home::medulla_home;

/// Production backend API base URL (the default).
pub const PROD_BACKEND_BASE_URL: &str = "https://api.tinyhumans.ai";
/// Staging backend API base URL (selected by `MEDULLA_STAGING`).
pub const STAGING_BACKEND_BASE_URL: &str = "https://staging-api.tinyhumans.ai";
/// Production tiny.place base URL (the default).
pub const PROD_TINYPLACE_BASE_URL: &str = "https://api.tiny.place";
/// Staging tiny.place base URL (selected by `MEDULLA_STAGING`).
pub const STAGING_TINYPLACE_BASE_URL: &str = "https://staging-api.tiny.place";

/// Whether `MEDULLA_STAGING` selects the staging defaults. Truthy is `"1"` or
/// `"true"` (case-insensitive, trimmed).
pub fn is_staging(env: &HashMap<String, String>) -> bool {
    matches!(
        env.get("MEDULLA_STAGING")
            .map(|v| v.trim().to_ascii_lowercase())
            .as_deref(),
        Some("1") | Some("true")
    )
}

/// The default backend base URL for this environment (staging vs prod).
pub fn default_backend_base_url(env: &HashMap<String, String>) -> String {
    if is_staging(env) {
        STAGING_BACKEND_BASE_URL.to_string()
    } else {
        PROD_BACKEND_BASE_URL.to_string()
    }
}

/// The default tiny.place base URL for this environment (staging vs prod).
pub fn default_tinyplace_base_url(env: &HashMap<String, String>) -> String {
    if is_staging(env) {
        STAGING_TINYPLACE_BASE_URL.to_string()
    } else {
        PROD_TINYPLACE_BASE_URL.to_string()
    }
}

/// Resolve the backend base URL. Order: `MEDULLA_API_URL` env override >
/// explicitly-configured `backend.baseUrl` > staging/prod default. `config_url`
/// is the value present in the config file (`None` when the key was absent), so
/// an explicit config value is never clobbered by the default.
pub fn resolve_backend_base_url(env: &HashMap<String, String>, config_url: Option<&str>) -> String {
    if let Some(value) = env
        .get("MEDULLA_API_URL")
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
    {
        return value.to_string();
    }
    if let Some(value) = config_url.map(str::trim).filter(|v| !v.is_empty()) {
        return value.to_string();
    }
    default_backend_base_url(env)
}

/// Resolve the tiny.place base URL for the `[tinyplace]` section. Order:
/// explicitly-configured `tinyplace.baseUrl` > staging/prod default. (The
/// `TINYPLACE_*`/`NEXT_PUBLIC_API_URL` env chain is applied later, at endpoint
/// resolution in [`crate::tinyplace_support::config`].)
pub fn resolve_tinyplace_base_url(
    env: &HashMap<String, String>,
    config_url: Option<&str>,
) -> String {
    if let Some(value) = config_url.map(str::trim).filter(|v| !v.is_empty()) {
        return value.to_string();
    }
    default_tinyplace_base_url(env)
}

fn d_state_dir() -> String {
    // Placeholder for `TuiConfig::default()` / bare deserialization; the real
    // value is `<medulla_home>/state`, filled in by `load_config`.
    "state".into()
}
fn d_tp_base() -> String {
    PROD_TINYPLACE_BASE_URL.into()
}
fn d_tp_identity() -> String {
    // Placeholder; the real value is `<medulla_home>/tinyplace`, filled in by
    // `load_config`.
    "tinyplace".into()
}
fn d_accept() -> String {
    "peers".into()
}
fn d_opencode_cmd() -> String {
    "opencode".into()
}
fn d_agent() -> String {
    "build".into()
}
fn d_workspace() -> String {
    ".".into()
}
fn d_concurrency() -> u32 {
    4
}
fn d_true() -> bool {
    true
}
fn d_backend_base() -> String {
    PROD_BACKEND_BASE_URL.into()
}
fn d_token_env() -> String {
    "MEDULLA_TOKEN".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct MedullaConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_passes: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tasks_per_delegate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u32>,
}

impl MedullaConfig {
    pub fn context_window(&self) -> u32 {
        self.context_window_tokens.unwrap_or(32_000)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "d_task_protocol")]
    pub protocol: String,
}

fn d_task_protocol() -> String {
    "task".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TinyplaceConfig {
    #[serde(default = "d_tp_base")]
    pub base_url: String,
    #[serde(default = "d_tp_identity")]
    pub identity_dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bio: Option<String>,
    #[serde(default = "d_true")]
    pub auto_discover_peers: bool,
    #[serde(default = "d_accept")]
    pub accept_contacts: String,
    pub peers: Vec<Peer>,
}

impl Default for TinyplaceConfig {
    fn default() -> Self {
        TinyplaceConfig {
            base_url: d_tp_base(),
            identity_dir: d_tp_identity(),
            handle: None,
            display_name: None,
            bio: None,
            auto_discover_peers: true,
            accept_contacts: d_accept(),
            peers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OpencodeConfig {
    #[serde(default = "d_opencode_cmd")]
    pub command: String,
    #[serde(default)]
    pub model: String,
    #[serde(default = "d_agent")]
    pub agent: String,
    #[serde(default = "d_workspace")]
    pub workspace: String,
    #[serde(default = "d_concurrency")]
    pub max_concurrency: u32,
}

impl Default for OpencodeConfig {
    fn default() -> Self {
        OpencodeConfig {
            command: d_opencode_cmd(),
            model: String::new(),
            agent: d_agent(),
            workspace: d_workspace(),
            max_concurrency: d_concurrency(),
        }
    }
}

/// Where the TUI reaches the core-js orchestration core (its NDJSON Unix socket).
/// An explicit `socketPath` overrides the env-based resolution
/// (`$XDG_RUNTIME_DIR/medulla/core.sock` → `<stateDir>/core.sock`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct CoreConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub socket_path: Option<String>,
}

/// The optional `memory` section: tinycortex persona memory integration. All
/// fields are optional overrides; the effective settings are resolved against
/// the environment in [`crate::memory::env`].
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct MemoryConfigSection {
    /// On/off switch (also settable via `MEDULLA_MEMORY`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Workspace root for the chunk store / facet trees / `persona/` outputs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Identity line for the compiled pack header.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    /// Claude Code transcript root override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_root: Option<String>,
    /// Codex rollout root override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex_root: Option<String>,
    /// Project roots walked for instruction files + git history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub project_roots: Vec<String>,
    /// Chat/digest model id override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Per-run provider spend ceiling, USD.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_cost_usd: Option<f64>,
}

/// Where the TUI reaches the Medulla backend HTTP API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BackendConfig {
    #[serde(default = "d_backend_base")]
    pub base_url: String,
    #[serde(default = "d_token_env")]
    pub token_env: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl Default for BackendConfig {
    fn default() -> Self {
        BackendConfig {
            base_url: d_backend_base(),
            token_env: d_token_env(),
            token: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TuiConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opencode: Option<OpencodeConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tinyplace: Option<TinyplaceConfig>,
    pub medulla: MedullaConfig,
    #[serde(default = "d_state_dir")]
    pub state_dir: String,
    pub backend: BackendConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub core: Option<CoreConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryConfigSection>,
}

impl Default for TuiConfig {
    fn default() -> Self {
        TuiConfig {
            opencode: None,
            tinyplace: None,
            medulla: MedullaConfig::default(),
            state_dir: d_state_dir(),
            backend: BackendConfig::default(),
            core: None,
            memory: None,
        }
    }
}

/// The parsed config alongside the path it is primarily identified by and the
/// ordered list of config files that actually contributed to it (low → high
/// precedence). `sources` is empty when only built-in defaults applied.
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: TuiConfig,
    pub path: String,
    pub sources: Vec<String>,
}

impl LoadedConfig {
    /// A defaults-only config, for a `--config` path that does not exist yet.
    pub fn defaults(path: String) -> Self {
        LoadedConfig {
            config: TuiConfig::default(),
            path,
            sources: Vec::new(),
        }
    }

    /// The harness label for the Agents view: `TINYPLACE` when tinyplace is
    /// configured, else the opencode command's basename uppercased.
    pub fn harness(&self) -> String {
        if self.config.tinyplace.is_some() {
            "TINYPLACE".into()
        } else if let Some(oc) = &self.config.opencode {
            oc.command
                .rsplit('/')
                .next()
                .filter(|s| !s.is_empty())
                .unwrap_or("worker")
                .to_uppercase()
        } else {
            "WORKER".into()
        }
    }

    /// Pretty-printed config JSON for the Config tab, with `backend.tokenEnv`
    /// annotated `<env> (set|missing)`.
    pub fn pretty_json(&self) -> String {
        let mut value = serde_json::to_value(&self.config).unwrap_or(Value::Null);
        let env = &self.config.backend.token_env;
        let set = std::env::var(env).ok().filter(|s| !s.is_empty()).is_some();
        if let Some(be) = value.get_mut("backend").and_then(|v| v.as_object_mut()) {
            be.insert(
                "tokenEnv".into(),
                Value::String(format!("{env} ({})", if set { "set" } else { "missing" })),
            );
        }
        serde_json::to_string_pretty(&value).unwrap_or_default()
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
fn merge_value(base: &mut Value, overlay: Value) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// A unique temp dir for a test, used as an injected `MEDULLA_HOME` and/or cwd.
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "medulla-cfg-{tag}-{}-{:p}",
            std::process::id(),
            &tag as *const _
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn defaults_are_applied() {
        // Serde defaults (no env resolution) produce the PROD urls and the
        // home-less state-dir placeholder (real value filled by load_config).
        let cfg: TuiConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.state_dir, "state");
        assert_eq!(cfg.backend.base_url, "https://api.tinyhumans.ai");
        assert_eq!(cfg.backend.token_env, "MEDULLA_TOKEN");
        assert_eq!(cfg.medulla.context_window(), 32_000);
    }

    #[test]
    fn backend_url_precedence() {
        // Nothing set → prod default.
        assert_eq!(
            resolve_backend_base_url(&env(&[]), None),
            "https://api.tinyhumans.ai"
        );
        // Staging switch flips the default.
        assert_eq!(
            resolve_backend_base_url(&env(&[("MEDULLA_STAGING", "1")]), None),
            "https://staging-api.tinyhumans.ai"
        );
        assert_eq!(
            resolve_backend_base_url(&env(&[("MEDULLA_STAGING", "TRUE")]), None),
            "https://staging-api.tinyhumans.ai"
        );
        // A non-truthy value keeps prod.
        assert_eq!(
            resolve_backend_base_url(&env(&[("MEDULLA_STAGING", "no")]), None),
            "https://api.tinyhumans.ai"
        );
        // Explicit config beats the (staging) default.
        assert_eq!(
            resolve_backend_base_url(&env(&[("MEDULLA_STAGING", "1")]), Some("http://x:1")),
            "http://x:1"
        );
        // MEDULLA_API_URL beats both config and default.
        assert_eq!(
            resolve_backend_base_url(
                &env(&[
                    ("MEDULLA_STAGING", "1"),
                    ("MEDULLA_API_URL", "http://env:2")
                ]),
                Some("http://x:1")
            ),
            "http://env:2"
        );
        // An empty MEDULLA_API_URL is ignored; config wins.
        assert_eq!(
            resolve_backend_base_url(&env(&[("MEDULLA_API_URL", "")]), Some("http://x:1")),
            "http://x:1"
        );
    }

    #[test]
    fn tinyplace_url_precedence() {
        assert_eq!(
            resolve_tinyplace_base_url(&env(&[]), None),
            "https://api.tiny.place"
        );
        assert_eq!(
            resolve_tinyplace_base_url(&env(&[("MEDULLA_STAGING", "true")]), None),
            "https://staging-api.tiny.place"
        );
        // Explicit config beats the staging default.
        assert_eq!(
            resolve_tinyplace_base_url(&env(&[("MEDULLA_STAGING", "1")]), Some("https://cfg")),
            "https://cfg"
        );
    }

    #[test]
    fn load_config_applies_staging_switch_to_both_urls() {
        let home = temp_dir("staging-home");
        let cwd = temp_dir("staging-cwd");
        let base_env = &[
            ("MEDULLA_HOME", home.to_str().unwrap()),
            ("MEDULLA_STAGING", "1"),
        ];
        // No config file + staging env → staging defaults for backend.
        let loaded = load_config(None, &env(base_env), &cwd).unwrap();
        assert_eq!(
            loaded.config.backend.base_url,
            "https://staging-api.tinyhumans.ai"
        );

        let cfg = cwd.join("medulla.tui.json");
        std::fs::write(&cfg, r#"{"tinyplace":{"peers":[]}}"#).unwrap();
        let loaded = load_config(Some(cfg.to_str().unwrap()), &env(base_env), &cwd).unwrap();
        assert_eq!(
            loaded.config.backend.base_url,
            "https://staging-api.tinyhumans.ai"
        );
        assert_eq!(
            loaded.config.tinyplace.unwrap().base_url,
            "https://staging-api.tiny.place"
        );
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn load_config_explicit_urls_win_over_env() {
        let home = temp_dir("explicit-home");
        let cwd = temp_dir("explicit-cwd");
        let cfg = cwd.join("medulla.tui.json");
        std::fs::write(
            &cfg,
            r#"{"backend":{"baseUrl":"http://be:1"},"tinyplace":{"baseUrl":"http://tp:2","peers":[]}}"#,
        )
        .unwrap();
        let home_env = ("MEDULLA_HOME", home.to_str().unwrap());
        // Staging set, but explicit config baseUrls win.
        let loaded = load_config(
            Some(cfg.to_str().unwrap()),
            &env(&[home_env, ("MEDULLA_STAGING", "1")]),
            &cwd,
        )
        .unwrap();
        assert_eq!(loaded.config.backend.base_url, "http://be:1");
        assert_eq!(loaded.config.tinyplace.unwrap().base_url, "http://tp:2");
        // But MEDULLA_API_URL still beats an explicit backend baseUrl.
        let loaded = load_config(
            Some(cfg.to_str().unwrap()),
            &env(&[home_env, ("MEDULLA_API_URL", "http://env:9")]),
            &cwd,
        )
        .unwrap();
        assert_eq!(loaded.config.backend.base_url, "http://env:9");
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn backend_and_tinyplace_parse() {
        let cfg: TuiConfig = serde_json::from_str(
            r#"{"backend":{"baseUrl":"http://x:1","token":"t"},"tinyplace":{"peers":[{"id":"p1","handle":"@a"}]}}"#,
        )
        .unwrap();
        assert_eq!(cfg.backend.base_url, "http://x:1");
        assert_eq!(cfg.backend.token.as_deref(), Some("t"));
        let tp = cfg.tinyplace.unwrap();
        assert_eq!(tp.peers.len(), 1);
        assert_eq!(tp.peers[0].protocol, "task");
        // Serde default (no env resolution) is the prod tiny.place URL.
        assert_eq!(tp.base_url, "https://api.tiny.place");
    }

    #[test]
    fn harness_label() {
        let mut loaded = LoadedConfig::defaults("x".into());
        loaded.config.opencode = Some(OpencodeConfig {
            command: "/usr/bin/opencode".into(),
            ..Default::default()
        });
        assert_eq!(loaded.harness(), "OPENCODE");
        loaded.config.tinyplace = Some(TinyplaceConfig::default());
        assert_eq!(loaded.harness(), "TINYPLACE");
    }

    #[test]
    fn pretty_json_annotates_token_env() {
        let loaded = LoadedConfig::defaults("x".into());
        let json = loaded.pretty_json();
        assert!(json.contains("MEDULLA_TOKEN ("));
    }

    #[test]
    fn pretty_json_marks_token_set_when_env_present() {
        let var = "MEDULLA_CONFIG_TEST_TOKEN";
        std::env::set_var(var, "value");
        let mut loaded = LoadedConfig::defaults("x".into());
        loaded.config.backend.token_env = var.into();
        assert!(loaded.pretty_json().contains(&format!("{var} (set)")));
        std::env::remove_var(var);
        assert!(loaded.pretty_json().contains(&format!("{var} (missing)")));
    }

    #[test]
    fn harness_defaults_to_worker_without_backends() {
        // No tinyplace and no opencode → the generic WORKER label.
        let loaded = LoadedConfig::defaults("x".into());
        assert_eq!(loaded.harness(), "WORKER");
    }

    #[test]
    fn harness_opencode_bare_command_and_empty() {
        let mut loaded = LoadedConfig::defaults("x".into());
        loaded.config.opencode = Some(OpencodeConfig {
            command: "codex".into(),
            ..Default::default()
        });
        assert_eq!(loaded.harness(), "CODEX");
        // A trailing-slash / empty basename falls back to WORKER.
        loaded.config.opencode = Some(OpencodeConfig {
            command: "bin/".into(),
            ..Default::default()
        });
        assert_eq!(loaded.harness(), "WORKER");
    }

    #[test]
    fn context_window_honors_override() {
        let cfg: TuiConfig =
            serde_json::from_str(r#"{"medulla":{"contextWindowTokens":128000}}"#).unwrap();
        assert_eq!(cfg.medulla.context_window(), 128_000);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        // Permissive parsing: extra keys (including retired sections like
        // `inference`/`langfuse`) must not fail the load.
        let cfg: TuiConfig = serde_json::from_str(
            r#"{"totallyUnknown":true,"inference":{"temperature":0.9},"langfuse":{"enabled":true},"medulla":{"maxPasses":3}}"#,
        )
        .unwrap();
        assert_eq!(cfg.medulla.max_passes, Some(3));
    }

    #[test]
    fn load_config_missing_file_yields_home_derived_defaults() {
        let home = temp_dir("nope-home");
        let cwd = temp_dir("nope-cwd");
        // No files anywhere → defaults, with state dir under <home>/state.
        let loaded = load_config(
            None,
            &env(&[("MEDULLA_HOME", home.to_str().unwrap())]),
            &cwd,
        )
        .unwrap();
        assert_eq!(
            loaded.config.state_dir,
            home.join("state").to_string_lossy()
        );
        assert_eq!(loaded.path, "(built-in defaults)");
        assert!(loaded.sources.is_empty());
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn load_config_reads_and_parses_a_file() {
        let home = temp_dir("reads-home");
        let dir = temp_dir("reads-cwd");
        let path = dir.join("medulla.tui.json");
        std::fs::write(&path, r#"{"stateDir":"/custom/state"}"#).unwrap();
        let loaded = load_config(
            Some(path.to_str().unwrap()),
            &env(&[("MEDULLA_HOME", home.to_str().unwrap())]),
            &dir,
        )
        .unwrap();
        // An explicit stateDir is preserved (not overridden by <home>/state).
        assert_eq!(loaded.config.state_dir, "/custom/state");
        assert_eq!(loaded.sources.len(), 1);
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_config_invalid_json_errors() {
        let dir = temp_dir("bad-cwd");
        let path = dir.join("bad.json");
        std::fs::write(&path, "{ this is not json").unwrap();
        let err = load_config(Some(path.to_str().unwrap()), &env(&[]), &dir).unwrap_err();
        assert!(err.to_string().contains("Invalid JSON"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_config_state_and_identity_derive_from_home() {
        let home = temp_dir("derive-home");
        let cwd = temp_dir("derive-cwd");
        // A tinyplace section with no identityDir → <home>/tinyplace; stateDir → <home>/state.
        let cfg = cwd.join("medulla.toml");
        std::fs::write(&cfg, "[tinyplace]\npeers = []\n").unwrap();
        let loaded = load_config(
            None,
            &env(&[("MEDULLA_HOME", home.to_str().unwrap())]),
            &cwd,
        )
        .unwrap();
        assert_eq!(
            loaded.config.state_dir,
            home.join("state").to_string_lossy()
        );
        assert_eq!(
            loaded.config.tinyplace.unwrap().identity_dir,
            home.join("tinyplace").to_string_lossy()
        );
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn load_config_state_dir_env_override_wins() {
        let home = temp_dir("stateenv-home");
        let cwd = temp_dir("stateenv-cwd");
        let loaded = load_config(
            None,
            &env(&[
                ("MEDULLA_HOME", home.to_str().unwrap()),
                ("MEDULLA_STATE_DIR", "/env/state"),
            ]),
            &cwd,
        )
        .unwrap();
        assert_eq!(loaded.config.state_dir, "/env/state");
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn load_config_layers_global_project_env_flag() {
        let home = temp_dir("layer-home");
        let cwd = temp_dir("layer-cwd");
        // Global config sets a base URL and a token env name.
        std::fs::write(
            home.join("config.toml"),
            "[backend]\nbaseUrl = \"http://global:1\"\ntokenEnv = \"GLOBAL_TOK\"\n",
        )
        .unwrap();
        // Project-local overrides just backend.baseUrl (field-level merge).
        std::fs::create_dir_all(cwd.join(".medulla")).unwrap();
        std::fs::write(
            cwd.join(".medulla").join("config.toml"),
            "[backend]\nbaseUrl = \"http://project:2\"\n",
        )
        .unwrap();

        // Global < project: project wins on baseUrl, global's tokenEnv survives.
        let loaded = load_config(
            None,
            &env(&[("MEDULLA_HOME", home.to_str().unwrap())]),
            &cwd,
        )
        .unwrap();
        assert_eq!(loaded.config.backend.base_url, "http://project:2");
        assert_eq!(loaded.config.backend.token_env, "GLOBAL_TOK");
        assert_eq!(loaded.sources.len(), 2);

        // Env beats both files.
        let loaded = load_config(
            None,
            &env(&[
                ("MEDULLA_HOME", home.to_str().unwrap()),
                ("MEDULLA_API_URL", "http://env:3"),
            ]),
            &cwd,
        )
        .unwrap();
        assert_eq!(loaded.config.backend.base_url, "http://env:3");
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn load_config_toml_and_json_parity() {
        let home = temp_dir("parity-home");
        let cwd = temp_dir("parity-cwd");
        let home_env = ("MEDULLA_HOME", home.to_str().unwrap());
        let json = cwd.join("c.json");
        std::fs::write(
            &json,
            r#"{"backend":{"baseUrl":"http://x:1"},"medulla":{"maxPasses":3}}"#,
        )
        .unwrap();
        let toml_path = cwd.join("c.toml");
        std::fs::write(
            &toml_path,
            "[backend]\nbaseUrl = \"http://x:1\"\n\n[medulla]\nmaxPasses = 3\n",
        )
        .unwrap();
        let from_json = load_config(Some(json.to_str().unwrap()), &env(&[home_env]), &cwd).unwrap();
        let from_toml =
            load_config(Some(toml_path.to_str().unwrap()), &env(&[home_env]), &cwd).unwrap();
        assert_eq!(from_json.config.backend.base_url, "http://x:1");
        assert_eq!(from_toml.config.backend.base_url, "http://x:1");
        assert_eq!(from_json.config.medulla.max_passes, Some(3));
        assert_eq!(from_toml.config.medulla.max_passes, Some(3));
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn merge_value_is_recursive() {
        let mut base = serde_json::json!({"a":{"x":1,"y":2},"b":9});
        merge_value(&mut base, serde_json::json!({"a":{"y":5,"z":3},"c":7}));
        assert_eq!(
            base,
            serde_json::json!({"a":{"x":1,"y":5,"z":3},"b":9,"c":7})
        );
    }

    #[test]
    fn memory_section_parses_camel_case() {
        let cfg: TuiConfig = serde_json::from_str(
            r#"{"memory":{"enabled":true,"workspace":"/ws","identity":"a@b","projectRoots":["/x","/y"],"model":"m","maxCostUsd":3.0}}"#,
        )
        .unwrap();
        let mem = cfg.memory.unwrap();
        assert_eq!(mem.enabled, Some(true));
        assert_eq!(mem.workspace.as_deref(), Some("/ws"));
        assert_eq!(mem.identity.as_deref(), Some("a@b"));
        assert_eq!(mem.project_roots, vec!["/x".to_string(), "/y".to_string()]);
        assert_eq!(mem.model.as_deref(), Some("m"));
        assert_eq!(mem.max_cost_usd, Some(3.0));
        // Absent by default.
        let bare: TuiConfig = serde_json::from_str("{}").unwrap();
        assert!(bare.memory.is_none());
    }

    #[test]
    fn peer_protocol_defaults_to_task() {
        let peer: Peer = serde_json::from_str(r#"{"id":"p1"}"#).unwrap();
        assert_eq!(peer.protocol, "task");
    }
}
