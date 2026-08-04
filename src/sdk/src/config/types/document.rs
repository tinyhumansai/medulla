//! The complete config document and its loaded provenance.

use super::*;

/// The whole parsed config document (`medulla.tui.json` / `medulla.toml`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TuiConfig {
    /// Optional local OpenCode harness configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opencode: Option<OpencodeConfig>,
    /// Optional tiny.place identity and discovery configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<LinkConfig>,
    /// Orchestration limits.
    pub medulla: MedullaConfig,
    /// Directory for runtime state and caches.
    #[serde(default = "d_state_dir")]
    pub state_dir: String,
    /// Backend HTTP API and authentication configuration.
    pub backend: BackendConfig,
    /// Optional embedded-core socket configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub core: Option<CoreConfig>,
    /// Release update-check configuration.
    #[serde(default)]
    pub update: UpdateConfig,
    /// TUI color overrides.
    #[serde(default)]
    pub theme: ThemeConfig,
    /// TUI display preferences.
    ///
    /// Superseded for harness rows by [`TuiConfig::status_line`]; the two
    /// `showHarness*` booleans here are still honoured as its starting point
    /// for a config written before that section existed.
    #[serde(default)]
    pub appearance: AppearanceConfig,
    /// How a harness row on the Agents rail is laid out. Absent means "derive
    /// it from `[appearance]`" — see [`TuiConfig::status_line`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_line: Option<StatusLineConfig>,
    /// Persisted welcome-flow state.
    #[serde(default)]
    pub onboarding: OnboardingConfig,
    /// Workspace roots managed by the daemon worker TUI.
    #[serde(default)]
    pub workflow: WorkflowConfig,
    /// What authored workflows may do when they run. Distinct from `workflow`
    /// above, which despite the name is only a list of workspace roots.
    #[serde(default)]
    pub workflows: WorkflowsConfig,
    /// What Medulla's MCP server offers the harnesses it spawns.
    #[serde(default)]
    pub mcp: McpSection,
    /// Persisted orchestrator worker roster.
    #[serde(default)]
    pub hub: HubSection,
    /// Whether this device also hosts the tasks the orchestrator hands out.
    #[serde(default)]
    pub host: HostSection,
    /// Additional hosts on this same machine, each working in its own directory.
    ///
    /// One laptop routinely holds several unrelated repos, and `[host]` can only
    /// name one working directory — its `workspaces` list is advisory, so a task
    /// still runs in the primary. Each entry here binds its own device-local
    /// address and registers as its own agent, so the orchestrator can be told
    /// *which project* to work in rather than only *which machine*.
    ///
    /// Additive: `[host]` keeps its meaning, and a config with no `[[hosts]]`
    /// behaves exactly as before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<HostSection>,
    /// How operator-started harnesses behave.
    #[serde(default)]
    pub harness: HarnessSection,
    /// Whether commits made by a Medulla-launched harness are attributed to
    /// Medulla. On by default.
    #[serde(default)]
    pub attribution: AttributionConfig,
    /// Lifecycle hooks Medulla installs into every harness it launches, declared
    /// once here rather than once per harness config. Empty by default.
    #[serde(default, skip_serializing_if = "HooksConfig::is_empty")]
    pub hooks: HooksConfig,
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
            link: None,
            medulla: MedullaConfig::default(),
            state_dir: d_state_dir(),
            backend: BackendConfig::default(),
            core: None,
            update: UpdateConfig::default(),
            theme: ThemeConfig::default(),
            appearance: AppearanceConfig::default(),
            status_line: None,
            onboarding: OnboardingConfig::default(),
            workflow: WorkflowConfig::default(),
            workflows: WorkflowsConfig::default(),
            mcp: McpSection::default(),
            hub: HubSection::default(),
            host: HostSection::default(),
            hosts: Vec::new(),
            harness: HarnessSection::default(),
            attribution: AttributionConfig::default(),
            hooks: HooksConfig::default(),
            router: None,
            budget: None,
            routing_strategy: None,
            subscription_routing_strategy: None,
            fleet: FleetConfig::default(),
        }
    }
}

impl TuiConfig {
    /// The effective harness status-line layout.
    ///
    /// Reads the explicit `[statusLine]` section when one is present, and
    /// otherwise derives one from the older `[appearance]` booleans so an
    /// upgrade preserves whatever the operator had already turned off. Returned
    /// by value — the config is small and `Copy` — so callers can hold it while
    /// borrowing the rest of the app mutably to draw.
    pub fn status_line(&self) -> StatusLineConfig {
        self.status_line
            .unwrap_or_else(|| StatusLineConfig::from_appearance(&self.appearance))
    }
}

/// The parsed config alongside the path it is primarily identified by and the
/// ordered list of config files that actually contributed to it (low → high
/// precedence). `sources` is empty when only built-in defaults applied.
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    /// Effective configuration after merging defaults and source files.
    pub config: TuiConfig,
    /// Primary path used to identify or persist the configuration.
    pub path: String,
    /// Contributing configuration files in ascending precedence order.
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

    /// The harness label for the Agents view: `LINK` when a host link is
    /// configured, else the opencode command's basename uppercased.
    pub fn harness(&self) -> String {
        if self.config.link.is_some() {
            "LINK".into()
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
