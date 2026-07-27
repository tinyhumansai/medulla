//! The config data model: every `[section]` the TUI reads, plus the
//! [`LoadedConfig`] result that pairs the parsed config with its provenance.
//!
//! Deserialization is permissive — missing fields take the `d_*` serde defaults
//! and unknown fields are ignored. Environment-dependent values (base URLs,
//! home-derived paths) are filled in afterwards by
//! [`load_config`](super::load_config), not here.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::urls::{PROD_BACKEND_BASE_URL, PROD_TINYPLACE_BASE_URL};
use crate::runtime::fleet::{
    AgentTemplate, CapacitySnapshot, HarnessDescriptor, HostDescriptor, WorkspaceDescriptor,
};
use crate::runtime::{AgentDescriptor, RoutingStrategy, SubscriptionRoutingStrategy};
use crate::tinyplace::BudgetWindow;

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

/// The `host` section: whether this machine also *runs* the work the
/// orchestrator hands out, and how.
///
/// A plain `medulla` is both orchestrator and host by default — the common case
/// is one person on one laptop, and needing a second `medulla daemon` process
/// beside the TUI to get any work done was a step nobody should have to know
/// about. Every field has a working default; the whole section is optional.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct HostSection {
    /// Whether to host tasks on this device. `MEDULLA_HOST=0` overrides it off
    /// for a single run without editing the file.
    pub enabled: bool,
    /// The address the host binds on the device-local bus. Orchestrator-facing
    /// only — it never leaves this machine — so it is a readable name rather
    /// than a key.
    pub address: String,
    /// Working directory tasks run in. Empty means the directory `medulla` was
    /// launched from, which is the one the operator is looking at.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace: String,
    /// Other directories this device is willing to work in.
    ///
    /// Advertised to the orchestrator as `capabilities.accessibleDirs`, so it
    /// knows this machine has more than one project on it rather than inferring
    /// the whole device from the folder `medulla` happened to be launched in.
    /// Advisory routing context: a delegated task still runs in
    /// [`workspace`](Self::workspace).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<String>,
    /// Coding-agent CLIs to serve. Empty detects whatever is installed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<String>,
    /// The CLI used when a task names none. Empty takes the first detected.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub default_provider: String,
    /// Maximum tasks run at once on this machine.
    pub concurrency: u32,
    /// Per-task execution timeout, in ms.
    pub task_timeout_ms: u64,
    /// Default model hint passed to the harness. Empty leaves the CLI's own.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// Whether the harness runs with its permission prompts bypassed. On by
    /// default because a hosted task is unattended: nobody is in the pane to
    /// answer a prompt, so a task that hits one has hung until it times out.
    pub skip_permissions: bool,
}

impl Default for HostSection {
    fn default() -> Self {
        HostSection {
            enabled: true,
            address: d_host_address(),
            workspace: String::new(),
            workspaces: Vec::new(),
            providers: Vec::new(),
            default_provider: String::new(),
            concurrency: 2,
            task_timeout_ms: 600_000,
            model: String::new(),
            skip_permissions: true,
        }
    }
}

fn d_host_address() -> String {
    "this-device".into()
}

/// Workspace roots a daemon worker may expose to operator-managed sessions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkflowConfig {
    /// Explicit local workspace roots available through the daemon TUI.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<String>,
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

/// Per-provider router override. Only `baseUrl` is provider-scoped today: the
/// three harnesses reach a custom endpoint differently (claude needs an
/// Anthropic-passthrough URL, codex/opencode an OpenAI-compatible one), so the
/// endpoint can be steered per provider while the API key stays shared.
///
/// Mirrors medulla-v1's `RouterProviderConfig` and the backend's stored shape so
/// one config document round-trips across all three modules.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RouterProviderConfig {
    /// OpenAI-compatible (or Anthropic-passthrough) endpoint for this provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

/// The optional `[router]` section: a custom OpenAI-compatible router (gateway or
/// proxy) the worker points its harnesses at, so inference is centralized,
/// metered, and re-routable without hand-editing each harness's on-disk config.
///
/// camelCase on the wire (`baseUrl`, `apiKeyEnv`, `models`,
/// `providers.<p>.baseUrl`), matching the published contract that medulla-v1
/// resolves and the backend serves at `GET/PUT /medulla/v1/router`. Absent
/// entirely means the feature is off — zero behaviour change.
///
/// The API key is referenced by env-var **name** (`apiKeyEnv`), never inlined:
/// it is resolved from the daemon's own environment at spawn and excluded from
/// every frame and config diagnostic. Every field is optional; a config that
/// sets only `baseUrl` still routes, deferring model selection to the harness.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RouterConfig {
    /// Top-level OpenAI-compatible endpoint for every provider without an override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Env-var NAME holding the API key — never the secret itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// Tier → model/SKU mapping (`reasoning` / `compress` / `orchestrator`). A
    /// missing tier falls through to the harness's own configuration.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub models: HashMap<String, String>,
    /// Per-provider overrides, keyed by provider id (`claude` / `codex` /
    /// `opencode`).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub providers: HashMap<String, RouterProviderConfig>,
}

impl RouterConfig {
    /// The effective endpoint for `provider`, applying the documented precedence
    /// `providers.<p>.baseUrl` > top-level `baseUrl` > (unset → the harness's
    /// own on-disk config). Returns `None` when neither is configured.
    ///
    /// A blank endpoint (`baseUrl = ""`) at either level is treated as unset:
    /// a blank provider override cannot shadow a valid top-level endpoint, and a
    /// blank top-level value does not enable routing with an empty `*_BASE_URL`
    /// (which would break otherwise-normal spawns). Matches how blank `apiKeyEnv`
    /// values are filtered elsewhere.
    pub fn base_url_for(&self, provider: &str) -> Option<&str> {
        self.providers
            .get(provider)
            .and_then(|p| p.base_url.as_deref())
            .filter(|s| !s.is_empty())
            .or(self.base_url.as_deref())
            .filter(|s| !s.is_empty())
    }

    /// The model/SKU mapped for `tier`, or `None` when the router leaves it to
    /// the harness.
    pub fn model_for_tier(&self, tier: &str) -> Option<&str> {
        self.models.get(tier).map(String::as_str)
    }
}

/// Operator-declared budget numbers for one provider in the `[budget]` section.
///
/// Mirrors the daemon's `ConfiguredBudget`: every field is optional, and when
/// present it makes the advertised `HarnessBudget` authoritative
/// (`source: configured`) instead of a best-effort estimate. camelCase on the
/// wire; `window` reuses the snake_case [`BudgetWindow`] values (`daily` /
/// `weekly` / `five_hour` / `unknown`) so one document round-trips across all
/// three modules.
///
/// No credential material lives here: `seat` is an opaque label only, never a
/// key or token, matching the frame contract that excludes secrets from budgets.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProviderBudgetConfig {
    /// Opaque seat/subscription label the operator recorded (never a credential).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seat: Option<String>,
    /// The metering window the allowance renews on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<BudgetWindow>,
    /// The configured allowance for the window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_tokens: Option<i64>,
    /// Consumption recorded so far in the window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_tokens: Option<i64>,
    /// Remaining allowance, when the operator records it directly rather than
    /// leaving it to be derived from `limitTokens - usedTokens`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_tokens: Option<i64>,
    /// Unix seconds until which the seat is parked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_until: Option<i64>,
}

/// The optional `[budget]` section: operator-declared per-provider token budgets.
///
/// Absent entirely (the default) means every installed, usable harness advertises
/// a best-effort `estimate` with no invented numbers. A `[budget.providers.<p>]`
/// entry promotes that provider's advertised descriptor to `source: configured`
/// with the operator's exact numbers, so a hosted orchestrator can size tasks
/// against a real allowance. Keyed by provider id (`claude` / `codex` /
/// `opencode`), mirroring `[router.providers.<p>]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BudgetConfig {
    /// Per-provider configured budgets, keyed by provider id.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub providers: HashMap<String, ProviderBudgetConfig>,
}

impl BudgetConfig {
    /// The operator's configured numbers for `provider` (its wire id), if any.
    pub fn for_provider(&self, provider: &str) -> Option<&ProviderBudgetConfig> {
        self.providers.get(provider)
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

/// Operator-declared capacity: the `Host → Harness → Workspace → Agent`
/// containment chain plus the agent templates that may be provisioned into it.
///
/// Declared, never probed — this is what the client *offers* the orchestrator
/// when it attaches to `medulla-serve`, and what the TUI's Fleet page renders
/// when no backend supplies a fleet of its own. The default declares only the
/// built-in coding template catalog; it provisions no agents and advertises no
/// host capacity. An explicit empty template list opts out of that catalog.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct FleetConfig {
    /// Machines this client declares.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<HostDescriptor>,
    /// Agent CLI runtimes installed on those machines.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub harnesses: Vec<HarnessDescriptor>,
    /// Folders those runtimes expose.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<WorkspaceDescriptor>,
    /// Durable agent identities deployed into them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<AgentDescriptor>,
    /// Kinds of agent that may be provisioned onto this chain.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub agent_templates: Vec<AgentTemplate>,
}

impl Default for FleetConfig {
    fn default() -> Self {
        Self {
            hosts: Vec::new(),
            harnesses: Vec::new(),
            workspaces: Vec::new(),
            agents: Vec::new(),
            agent_templates: crate::agents::default_templates(),
        }
    }
}

impl FleetConfig {
    /// Whether the operator declared nothing at all.
    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
            && self.harnesses.is_empty()
            && self.workspaces.is_empty()
            && self.agents.is_empty()
            && self.agent_templates.is_empty()
    }

    /// Whether a catalog is all this config declares — no hosts, harnesses,
    /// workspaces, or agents.
    ///
    /// A template catalog says what *may* be provisioned, never where. So a
    /// config carrying only one has declared no fleet, and the opt-in demo
    /// fleet may still stand in: this is what lets the built-in catalog (or an
    /// installed `.medulla/agents` store) coexist with `MEDULLA_DEMO_FLEET`
    /// instead of suppressing it.
    pub fn declares_only_templates(&self) -> bool {
        self.hosts.is_empty()
            && self.harnesses.is_empty()
            && self.workspaces.is_empty()
            && self.agents.is_empty()
    }

    /// The declared chain as the UI-facing roll-up (agents excluded — they reach
    /// the UI through the snapshot roster).
    pub fn capacity(&self) -> CapacitySnapshot {
        CapacitySnapshot {
            hosts: self.hosts.clone(),
            harnesses: self.harnesses.clone(),
            workspaces: self.workspaces.clone(),
            templates: self.agent_templates.clone(),
        }
    }
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
    /// Workspace roots managed by the daemon worker TUI.
    #[serde(default)]
    pub workflow: WorkflowConfig,
    #[serde(default)]
    pub hub: HubSection,
    /// Whether this device also hosts the tasks the orchestrator hands out.
    #[serde(default)]
    pub host: HostSection,
    /// Custom OpenAI-compatible router. Absent means routing is off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub router: Option<RouterConfig>,
    /// Operator-declared per-provider token budgets. Absent means every harness
    /// advertises a best-effort estimate instead of configured numbers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<BudgetConfig>,
    /// The operator's persisted worker routing strategy (camelCase on the wire).
    /// Loaded on start and reconciled with the backend's routing-strategy config
    /// when present. Absent means no local preference (defaults to `manual`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing_strategy: Option<RoutingStrategy>,
    /// How the orchestrator chooses among ready provider subscriptions after
    /// selecting a host. Absent preserves the requested or host-default provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription_routing_strategy: Option<SubscriptionRoutingStrategy>,
    /// Operator-declared hosts, harnesses, workspaces, agents, and templates.
    #[serde(default, skip_serializing_if = "FleetConfig::is_empty")]
    pub fleet: FleetConfig,
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
            host: HostSection::default(),
            router: None,
            budget: None,
            routing_strategy: None,
            subscription_routing_strategy: None,
            fleet: FleetConfig::default(),
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
