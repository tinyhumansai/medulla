//! The config data model: every `[section]` the TUI reads, plus the
//! [`LoadedConfig`] result that pairs the parsed config with its provenance.
//!
//! Deserialization is permissive — missing fields take the `d_*` serde defaults
//! and unknown fields are ignored. Environment-dependent values (base URLs,
//! home-derived paths) are filled in afterwards by
//! [`load_config`](super::load_config), not here.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::urls::{PROD_BACKEND_BASE_URL, PROD_TINYPLACE_BASE_URL};

// --- serde default helpers -------------------------------------------------

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
fn d_task_protocol() -> String {
    "task".into()
}
fn d_max_lanes() -> u32 {
    4
}
fn d_shared_path_denylist() -> Vec<String> {
    vec![
        "**/Cargo.lock".into(),
        "**/package-lock.json".into(),
        "**/pnpm-lock.yaml".into(),
        "**/yarn.lock".into(),
        "**/generated/**".into(),
    ]
}

// --- config sections -------------------------------------------------------

/// The `medulla` orchestration limits section.
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
    /// The effective context window in tokens (default 32k).
    pub fn context_window(&self) -> u32 {
        self.context_window_tokens.unwrap_or(32_000)
    }
}

/// One statically-configured tiny.place peer.
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

/// One worker in the persisted `[hub]` roster.
///
/// Named apart from [`crate::hub::HubWorker`] on purpose: that is the live
/// in-memory entry the hub dispatches through, this is its on-disk form. They
/// carry the same fields today and are still different things — one survives a
/// restart, the other does not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubWorkerConfig {
    /// The `agentId` the backend targets.
    pub id: String,
    /// tiny.place address (base58 cryptoId or `@handle`).
    pub address: String,
    /// Coding-agent harness the worker runs.
    pub harness: String,
    /// Optional human label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Whether this worker is the selected default.
    #[serde(default)]
    pub selected: bool,
}

/// The `hub` section: the orchestrator's worker roster, remembered across runs.
///
/// Without this the roster lived only in memory, seeded from the environment at
/// boot — so a worker added from the Workers tab vanished at exit and the tab
/// was empty on the next launch however many peers were actually reachable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HubSection {
    /// Workers the operator has added, in roster order.
    pub workers: Vec<HubWorkerConfig>,
}

/// The `tinyplace` section: identity, discovery, and the static peer roster.
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

/// The `opencode` section: the local worker harness command and its defaults.
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

/// Local coding-workflow workspaces and advisory lane policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkflowConfig {
    /// Explicit local repository roots. An empty list falls back to the worker
    /// workspace configured under `[opencode]`, then the current directory.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<String>,
    /// Advisory maximum visible worker lanes. Engine limits remain authoritative.
    #[serde(default = "d_max_lanes")]
    pub max_lanes: u32,
    /// Glob-like paths that require coordination before concurrent edits.
    #[serde(default = "d_shared_path_denylist")]
    pub shared_path_denylist: Vec<String>,
}

impl Default for WorkflowConfig {
    fn default() -> Self {
        Self {
            workspaces: Vec::new(),
            max_lanes: d_max_lanes(),
            shared_path_denylist: d_shared_path_denylist(),
        }
    }
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

/// The `update` section: the periodic release-update check. Disabled entirely
/// by `check = false` here, or by the `MEDULLA_NO_UPDATE_CHECK=1` environment
/// variable (see [`UpdateConfig::enabled`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UpdateConfig {
    /// Whether the background TUI update check runs. Defaults to `true`.
    #[serde(default = "d_true")]
    pub check: bool,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        UpdateConfig { check: true }
    }
}

impl UpdateConfig {
    /// The effective on/off state: config `check` gated by the env kill-switch
    /// `MEDULLA_NO_UPDATE_CHECK` (any non-empty, non-`0` value disables it).
    pub fn enabled(&self, env: &HashMap<String, String>) -> bool {
        let killed = env
            .get("MEDULLA_NO_UPDATE_CHECK")
            .map(|v| !v.is_empty() && v != "0")
            .unwrap_or(false);
        self.check && !killed
    }
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

/// The optional `core` section: the NDJSON `medulla-serve` orchestration socket.
///
/// When configured, the core runtime attaches to a long-lived `medulla-serve`
/// process over a unix domain socket (the `medulla-serve` protocol, plan §2.2).
/// This milestone is attach-only: the socket must already be listening. The
/// section is unix-only; on Windows a request for it degrades to the
/// backend→mock chain (see [`super::load_config`]).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct CoreConfig {
    /// Explicit NDJSON unix socket path. When unset the socket is resolved from
    /// `$XDG_RUNTIME_DIR/medulla/serve.sock`, then `<stateDir>/serve.sock` (see
    /// [`LoadedConfig::core_socket_path`]).
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

/// The optional `[theme]` config section: named ratatui colors (case-insensitive)
/// or `#rrggbb` hex strings. Missing fields fall back to the default theme. The
/// Appearance settings subpage persists these keys.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ThemeConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_fg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dim_border: Option<String>,
}

/// Onboarding state: what the welcome flow has already shown this user.
///
/// Purely a display gate. Whether the user actually *earned* the history reward
/// is the backend's answer (`GET /agent-integrations/history-rewards/status`);
/// this flag only stops the welcome screen reappearing every launch, including
/// for a user who deliberately skipped it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct OnboardingConfig {
    /// True once the user has completed or skipped the welcome flow.
    pub welcome_completed: bool,
}

/// The whole parsed config document (`medulla.tui.json` / `medulla.toml`).
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
    #[serde(default)]
    pub update: UpdateConfig,
    #[serde(default)]
    pub theme: ThemeConfig,
    #[serde(default)]
    pub onboarding: OnboardingConfig,
    /// Local repository views and lane-coordination policy.
    #[serde(default)]
    pub workflow: WorkflowConfig,
    #[serde(default)]
    pub hub: HubSection,
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
            update: UpdateConfig::default(),
            theme: ThemeConfig::default(),
            onboarding: OnboardingConfig::default(),
            workflow: WorkflowConfig::default(),
            hub: HubSection::default(),
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

    /// Effective local workspace roots for workflow tooling.
    ///
    /// Explicit `[workflow].workspaces` win. Otherwise the configured worker
    /// workspace is used, falling back to the process current directory.
    pub fn workflow_workspaces(&self) -> Vec<PathBuf> {
        if !self.config.workflow.workspaces.is_empty() {
            return self
                .config
                .workflow
                .workspaces
                .iter()
                .map(PathBuf::from)
                .collect();
        }
        vec![PathBuf::from(
            self.config
                .opencode
                .as_ref()
                .map(|config| config.workspace.as_str())
                .unwrap_or("."),
        )]
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
