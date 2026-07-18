//! `medulla.tui.json`-compatible config — the subset the TUI reads, plus a
//! `backend` section for the HTTP runtime. Permissive: missing fields take
//! defaults, unknown fields are ignored.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

fn d_state_dir() -> String {
    ".medulla-state/tui".into()
}
fn d_tp_base() -> String {
    "https://staging-api.tiny.place".into()
}
fn d_tp_identity() -> String {
    ".medulla-state/tui-tinyplace".into()
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
    "http://localhost:5000".into()
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
        }
    }
}

/// The parsed config alongside the absolute path it came from.
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: TuiConfig,
    pub path: String,
}

impl LoadedConfig {
    /// A defaults-only config, for a `--config` path that does not exist yet.
    pub fn defaults(path: String) -> Self {
        LoadedConfig {
            config: TuiConfig::default(),
            path,
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

/// Load and parse the TUI config from `path`. A missing file yields defaults; a
/// present-but-invalid file is an error.
pub fn load_config(path: &str) -> anyhow::Result<LoadedConfig> {
    let absolute = std::fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| {
            Path::new(path)
                .to_str()
                .map(str::to_string)
                .unwrap_or_else(|| path.to_string())
        });
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LoadedConfig::defaults(absolute));
        }
        Err(err) => {
            return Err(anyhow::anyhow!(
                "Cannot read TUI config at {absolute}: {err}"
            ))
        }
    };
    let config: TuiConfig = serde_json::from_str(&text)
        .map_err(|err| anyhow::anyhow!("Invalid JSON in {absolute}: {err}"))?;
    Ok(LoadedConfig {
        config,
        path: absolute,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_applied() {
        let cfg: TuiConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.state_dir, ".medulla-state/tui");
        assert_eq!(cfg.backend.base_url, "http://localhost:5000");
        assert_eq!(cfg.backend.token_env, "MEDULLA_TOKEN");
        assert_eq!(cfg.medulla.context_window(), 32_000);
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
        assert_eq!(tp.base_url, "https://staging-api.tiny.place");
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
    fn load_config_missing_file_yields_defaults() {
        let path = std::env::temp_dir()
            .join(format!("medulla-nope-{}.json", std::process::id()))
            .to_string_lossy()
            .into_owned();
        let loaded = load_config(&path).unwrap();
        // Defaults applied; the absolute-ish path is preserved.
        assert_eq!(loaded.config.state_dir, ".medulla-state/tui");
        assert!(loaded.path.contains("medulla-nope-"));
    }

    #[test]
    fn load_config_reads_and_parses_a_file() {
        let dir = std::env::temp_dir().join(format!("medulla-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("medulla.tui.json");
        std::fs::write(&path, r#"{"stateDir":"/custom/state"}"#).unwrap();
        let loaded = load_config(path.to_str().unwrap()).unwrap();
        assert_eq!(loaded.config.state_dir, "/custom/state");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_config_invalid_json_errors() {
        let dir = std::env::temp_dir().join(format!("medulla-cfg-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.json");
        std::fs::write(&path, "{ this is not json").unwrap();
        let err = load_config(path.to_str().unwrap()).unwrap_err();
        assert!(err.to_string().contains("Invalid JSON"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn peer_protocol_defaults_to_task() {
        let peer: Peer = serde_json::from_str(r#"{"id":"p1"}"#).unwrap();
        assert_eq!(peer.protocol, "task");
    }
}
