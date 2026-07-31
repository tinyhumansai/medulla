//! Orchestrator, peer, hub, harness, host, and workflow configuration types.

use super::*;

// --- config sections -------------------------------------------------------

/// The `medulla` orchestration limits section.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct MedullaConfig {
    /// Maximum cognitive passes allowed in one orchestration run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_passes: Option<u32>,
    /// Maximum execution steps allowed in one orchestration run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<u32>,
    /// Maximum nested delegation depth.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u32>,
    /// Maximum tasks one delegate may create.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tasks_per_delegate: Option<u32>,
    /// Maximum aggregate token budget for a run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Model context window used when calculating prompt budgets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u32>,
}

/// The context window assumed when the operator has not set one.
///
/// One million tokens: the window of the models this actually runs against.
/// The old 32k default predates them, and understating the window is the
/// damaging direction — the meter reads near-full on a prompt using three
/// percent of the real budget, which is an alarm that fires constantly and so
/// gets ignored when it finally matters.
///
/// Still only a default. `contextWindowTokens` overrides it for a deployment
/// pointed at a smaller model.
pub const DEFAULT_CONTEXT_WINDOW_TOKENS: u32 = 1_000_000;

impl MedullaConfig {
    /// The effective context window in tokens, defaulting to
    /// [`DEFAULT_CONTEXT_WINDOW_TOKENS`].
    pub fn context_window(&self) -> u32 {
        self.context_window_tokens
            .unwrap_or(DEFAULT_CONTEXT_WINDOW_TOKENS)
    }
}

/// One statically-configured tiny.place peer.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    /// Stable peer identifier.
    pub id: String,
    /// Optional human-facing name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional tiny.place handle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    /// Address used to contact the peer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// Operator-defined labels used for discovery or routing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// Optional description shown beside the peer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Wire protocol used when sending tasks to this peer.
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
    /// Agent-template ids this worker is offered for. Empty means unspecified.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
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
    /// Base URL of the tiny.place service.
    #[serde(default = "d_tp_base")]
    pub base_url: String,
    /// Directory containing this installation's tiny.place identity.
    #[serde(default = "d_tp_identity")]
    pub identity_dir: String,
    /// Optional public handle registered for the identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    /// Optional public display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Optional public profile biography.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bio: Option<String>,
    /// Whether startup discovers peers from the service.
    #[serde(default = "d_true")]
    pub auto_discover_peers: bool,
    /// Contact acceptance policy (`peers`, for example).
    #[serde(default = "d_accept")]
    pub accept_contacts: String,
    /// Peers declared directly in configuration.
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
    /// OpenCode executable or command name.
    #[serde(default = "d_opencode_cmd")]
    pub command: String,
    /// Default model passed to OpenCode, or empty to use its default.
    #[serde(default)]
    pub model: String,
    /// Default OpenCode agent profile.
    #[serde(default = "d_agent")]
    pub agent: String,
    /// Default workspace for local OpenCode runs.
    #[serde(default = "d_workspace")]
    pub workspace: String,
    /// Maximum concurrent OpenCode tasks.
    #[serde(default = "d_concurrency")]
    pub max_concurrency: u32,
}

/// The `attribution` section: whether commits made by a Medulla-launched
/// harness say so.
///
/// Attribution is a `Co-authored-by` trailer on the commit *message*, not a
/// change of git author or committer identity, so blame, `git log --author`,
/// and the GitHub contribution graph are unaffected.
///
/// On by default: a harness commit that does not name the tool that wrote it is
/// the surprising case, not the other way round. Turn it off with
/// `[attribution] commit = false`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct AttributionConfig {
    /// Whether commits carry the `Co-authored-by: Medulla` trailer.
    #[serde(default = "d_true")]
    pub commit: bool,
}

impl Default for AttributionConfig {
    fn default() -> Self {
        AttributionConfig { commit: true }
    }
}

/// The `harness` section: how harnesses the operator starts themselves behave.
///
/// Distinct from [`HostSection`], which is about the harnesses the *orchestrator*
/// starts to serve tasks. These are the ones an operator opens by hand and the
/// orchestrator is not allowed to touch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct HarnessSection {
    /// What releasing the keyboard does when the operator holds the harness:
    /// `ask` (the default), `always`, or `never`.
    ///
    /// `ask` prompts, the way an editor prompts about unsaved changes — an
    /// operator who took a harness over and walked away has locked the
    /// orchestrator out of it, and releasing the keyboard is the last moment
    /// they are certainly thinking about it.
    pub handback: String,
    /// Whether an operator-started harness launches with its provider's
    /// permission-bypass flag.
    ///
    /// Off by default, and deliberately the opposite of the host's own
    /// `skipPermissions`: that flag exists because an unattended session hangs
    /// on a permission dialog, and this session is by definition attended.
    pub skip_permissions: bool,
    /// Recently chosen directories in the manual harness launcher, newest first.
    ///
    /// This is picker history, not a routing allowlist: it never changes where
    /// orchestrated tasks may run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_workspaces: Vec<String>,
}

impl Default for HarnessSection {
    fn default() -> Self {
        HarnessSection {
            handback: "ask".to_string(),
            skip_permissions: false,
            recent_workspaces: Vec::new(),
        }
    }
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
    /// What to call this host in the roster. Empty means "this device", which
    /// is right for the primary and wrong for every extra: several hosts on one
    /// machine differ only by where they work, so the directory is the name
    /// worth showing.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
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
            name: String::new(),
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
///
/// Named for the `workflow` key it parses, which predates the workflow engine
/// and has nothing to do with it — authored workflows are configured by
/// [`WorkflowsConfig`] under the plural `workflows` key.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkflowConfig {
    /// Explicit local workspace roots available through the daemon TUI.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<String>,
}

/// The `workflows` section: what authored workflows may do when they run.
///
/// Code execution defaults on so locally authored workflows work without an
/// extra host bootstrap step. Outbound HTTP and third-party tools remain
/// deny-by-default, and an operator can explicitly set `allowCode = false` for
/// untrusted workflow collections.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkflowsConfig {
    /// Whether workflows may be listed and run at all.
    #[serde(default = "d_true")]
    pub enabled: bool,
    /// The worker `agent` nodes dispatch to when they name no `agentRef`.
    /// Empty means a node must name one, and a run fails saying so.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default_worker: String,
    /// The harness hint sent with each dispatch. Absent uses the worker's own
    /// default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<HarnessProvider>,
    /// The model hint sent with each dispatch.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default_model: String,
    /// Whether `code` nodes may execute. This host has no sandbox, so enabling
    /// it grants a workflow author the daemon's own privileges.
    #[serde(default = "d_true")]
    pub allow_code: bool,
    /// Tool slugs a `tool_call` node may invoke, beyond the built-in
    /// `medulla:` operations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_allowlist: Vec<String>,
    /// Hosts an `http_request` node may reach. A bare domain also permits its
    /// subdomains.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub http_allowlist: Vec<String>,
    /// How long one run may take before the host abandons it.
    #[serde(default = "d_run_timeout_secs")]
    pub run_timeout_secs: u64,
    /// The most harness tasks one run may have in flight at once.
    ///
    /// A node can fan out over its input (`execution: "per_item"` with a
    /// `concurrency`), and here every item is a whole harness session. This is
    /// sized by what the worker pool can actually serve, so a graph asking for
    /// more is throttled to it rather than refused.
    #[serde(default = "d_max_parallel_agents")]
    pub max_parallel_agents: usize,
    /// Whether and how a workflow reviews its own history.
    #[serde(default)]
    pub evolve: EvolveSettings,
}

/// The `workflows.evolve` section: reviewing a workflow against its own runs.
///
/// On by default, unlike the capability switches above, because a pass cannot
/// change anything: it records notes and produces proposals an operator has to
/// accept. `autoOnFailure` is the one worth turning off — it spends a harness
/// session per failed workflow, which an operator running many may not want.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EvolveSettings {
    /// Whether a review may run at all.
    #[serde(default = "d_true")]
    pub enabled: bool,
    /// Whether a failed run starts one by itself.
    #[serde(default = "d_true")]
    pub auto_on_failure: bool,
    /// How many recent runs reach the review's brief.
    #[serde(default = "d_evolve_max_runs")]
    pub max_runs: usize,
    /// How many current notes reach the review's brief.
    #[serde(default = "d_evolve_max_notes")]
    pub max_notes: usize,
}

/// Enough runs to see a pattern, few enough that the brief is still mostly the
/// graph and the notes.
fn d_evolve_max_runs() -> usize {
    5
}

/// Well under the journal's cap: a review that reads a hundred notes is one
/// whose brief is mostly history.
fn d_evolve_max_notes() -> usize {
    40
}

impl Default for EvolveSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_on_failure: true,
            max_runs: d_evolve_max_runs(),
            max_notes: d_evolve_max_notes(),
        }
    }
}

/// A run may take ten minutes: long enough for real work on a coding harness,
/// short enough that a wedged run does not pin its record forever.
fn d_max_parallel_agents() -> usize {
    crate::flow_engine::DEFAULT_MAX_PARALLEL_AGENTS
}

fn d_run_timeout_secs() -> u64 {
    600
}

impl Default for WorkflowsConfig {
    fn default() -> Self {
        WorkflowsConfig {
            enabled: true,
            default_worker: String::new(),
            default_provider: None,
            default_model: String::new(),
            allow_code: true,
            tool_allowlist: Vec::new(),
            http_allowlist: Vec::new(),
            run_timeout_secs: d_run_timeout_secs(),
            max_parallel_agents: d_max_parallel_agents(),
            evolve: EvolveSettings::default(),
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
