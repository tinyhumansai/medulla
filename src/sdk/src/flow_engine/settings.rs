//! What the capability adapters are allowed to do, and where they keep things.
//!
//! Kept as a plain struct rather than read from [`crate::config`] directly so
//! the adapters stay unit-testable without constructing a whole client config,
//! and so the security-relevant defaults live in one readable place. The config
//! layer converts into this; nothing else should.

use std::path::PathBuf;

use crate::tinyplace::HarnessProvider;

/// Settings governing one workflow run's capabilities.
#[derive(Debug, Clone)]
pub struct CapabilitySettings {
    /// Whether workflows may run at all on this host.
    ///
    /// An operator's off switch. Listing and validating a workflow is still
    /// allowed when this is false — an operator turning the feature off should
    /// still be able to see and repair what is installed — but nothing
    /// executes.
    pub enabled: bool,
    /// Where [`StateStore`](tinyflows::caps::StateStore) keys are persisted.
    pub state_dir: PathBuf,
    /// Where the engine's run checkpoints live. Checkpoints are what make an
    /// approval pause survive a restart.
    pub checkpoint_dir: PathBuf,
    /// The bridge address of the worker an `agent` node dispatches to when its
    /// `agent_ref` names no more specific one.
    pub default_worker_address: String,
    /// The harness an `agent` node runs on when neither the node nor the
    /// workflow names one.
    pub default_provider: Option<HarnessProvider>,
    /// The custom harness preset an `agent` node runs on when neither the node
    /// nor the workflow names a harness.
    ///
    /// Never set from operator config — `workflows.defaultProvider` only names
    /// built-in harnesses. It exists so a workflow's own `defaults` block can
    /// pin a preset for every node in the graph, which is written into a run's
    /// settings by [`crate::workflows::run_workflow`].
    pub default_custom_harness: Option<String>,
    /// The model hint passed with each dispatch, when the host pins one.
    pub default_model: Option<String>,
    /// Whether `code` nodes may execute. On by default for local workflows,
    /// though this host has no sandbox and runs scripts with the daemon's full
    /// privileges.
    pub allow_code: bool,
    /// Tool slugs a `tool_call` node may invoke. Deny-by-default — an empty
    /// list permits only the built-in `medulla:` operations.
    pub tool_allowlist: Vec<String>,
    /// Hosts an `http_request` node may reach. Empty means "no outbound HTTP",
    /// so a workflow cannot become an exfiltration path by default.
    pub http_allowlist: Vec<String>,
    /// How long one run may take before it is abandoned.
    pub run_timeout_secs: u64,
    /// The most harness tasks one run may have in flight at once.
    ///
    /// A node can fan out over its input (`execution: "per_item"` with a
    /// `concurrency`), and on this host every one of those items is a whole
    /// harness session, not a model call. The engine bounds a *single* node's
    /// width; this bounds the run, because several nodes can be fanning out at
    /// the same time. Without it a workflow could dispatch a hundred sessions
    /// at once to a worker pool sized for a handful.
    ///
    /// An over-wide fan-out waits for a slot rather than failing, so raising a
    /// node's `concurrency` past this slows the run down instead of breaking
    /// it.
    pub max_parallel_agents: usize,
    /// The directory a `medulla:shell` script runs in.
    ///
    /// The operator's project, normally: a step that shells out almost always
    /// means to touch the repository the workflow is about. Empty means the
    /// host did not set one, and a script falls back to its own temporary
    /// directory rather than to whatever the daemon happened to be started in.
    pub workspace: String,
}

/// A run may take ten minutes before the host gives up on it. Matches the
/// sibling host's bound; long enough for a real coding task, short enough that a
/// wedged run does not pin its record forever.
pub const DEFAULT_RUN_TIMEOUT_SECS: u64 = 600;

/// Four harness tasks in flight per run by default.
///
/// Deliberately modest: a harness task is a full coding session, so the useful
/// ceiling is set by how many a worker can actually serve at once, not by how
/// many a graph feels like asking for. An operator with a bigger pool raises it.
pub const DEFAULT_MAX_PARALLEL_AGENTS: usize = 4;

impl CapabilitySettings {
    /// Settings rooted under a Medulla home.
    ///
    /// Local code execution is available by default. Outbound HTTP and
    /// third-party tools remain off until the operator allowlists them.
    pub fn rooted_at(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        let base = home.join("state").join("workflows");
        Self {
            enabled: true,
            state_dir: base.join("state"),
            checkpoint_dir: base.join("checkpoints"),
            default_worker_address: String::new(),
            default_provider: None,
            default_custom_harness: None,
            default_model: None,
            allow_code: true,
            tool_allowlist: Vec::new(),
            http_allowlist: Vec::new(),
            run_timeout_secs: DEFAULT_RUN_TIMEOUT_SECS,
            max_parallel_agents: DEFAULT_MAX_PARALLEL_AGENTS,
            workspace: String::new(),
        }
    }

    /// Fallback policy for a host whose operator configuration could not load.
    ///
    /// This is deliberately stricter than the ordinary local default: a
    /// malformed explicit opt-out must never turn code execution back on.
    pub fn fail_closed_at(home: impl Into<PathBuf>) -> Self {
        let mut settings = Self::rooted_at(home);
        settings.allow_code = false;
        settings
    }

    /// How long one script may run.
    ///
    /// A fraction of the run's own bound, so a wedged script fails as a *script*
    /// — naming itself in the error — rather than by silently consuming the
    /// whole run's budget and failing as a timeout with nothing to point at.
    /// The floor exists to keep a short quarter-share from being unreasonably
    /// tiny, but it is capped at the run's own timeout: for any
    /// `run_timeout_secs` under `4 × FLOOR_SECS` the uncapped floor would have
    /// exceeded the run's whole budget, which is exactly the failure mode this
    /// method exists to prevent.
    pub fn script_timeout(&self) -> std::time::Duration {
        const SHARE: u64 = 4;
        const FLOOR_SECS: u64 = 30;
        let share = (self.run_timeout_secs / SHARE).max(FLOOR_SECS);
        std::time::Duration::from_secs(share.min(self.run_timeout_secs.max(1)))
    }

    /// This run's standing harness preference — the layer under every node.
    ///
    /// Built from the host's config, then overwritten in place by a workflow's
    /// own `defaults` block when it has one, so an `agent` node only ever has
    /// two layers to consider: its own config and this.
    pub fn harness_preference(&self) -> crate::flow_engine::HarnessPreference {
        crate::flow_engine::HarnessPreference {
            harness: match (&self.default_custom_harness, self.default_provider) {
                // A named preset wins: only a workflow's own defaults can set
                // it, which is more specific than anything host config says.
                (Some(id), _) => Some(crate::flow_engine::HarnessSelector::Custom(id.clone())),
                (None, Some(provider)) => {
                    Some(crate::flow_engine::HarnessSelector::Builtin(provider))
                }
                (None, None) => None,
            },
            model: self.default_model.clone(),
        }
    }

    /// Whether `host` is permitted for outbound HTTP.
    ///
    /// Matches on exact host or a dot-suffix, so `example.com` permits
    /// `api.example.com` but not `notexample.com`.
    pub fn http_host_allowed(&self, host: &str) -> bool {
        let host = host.trim().to_ascii_lowercase();
        self.http_allowlist.iter().any(|allowed| {
            let allowed = allowed.trim().to_ascii_lowercase();
            host == allowed || host.ends_with(&format!(".{allowed}"))
        })
    }

    /// Whether a `tool_call` may invoke `slug`.
    pub fn tool_allowed(&self, slug: &str) -> bool {
        self.tool_allowlist.iter().any(|allowed| allowed == slug)
    }

    /// Settings from the operator's `workflows` config section, rooted under
    /// `home`.
    ///
    /// The only place config becomes capability policy. Everything else in the
    /// seam reads this struct, so there is one answer to "what is this run
    /// allowed to do" rather than a config lookup at each call site.
    pub fn from_config(config: &crate::config::WorkflowsConfig, home: impl Into<PathBuf>) -> Self {
        let mut settings = Self::rooted_at(home);
        settings.enabled = config.enabled;
        settings.default_worker_address = config.default_worker.clone();
        settings.default_provider = config.default_provider;
        settings.default_model =
            (!config.default_model.is_empty()).then(|| config.default_model.clone());
        settings.allow_code = config.allow_code;
        settings.tool_allowlist = config.tool_allowlist.clone();
        settings.http_allowlist = config.http_allowlist.clone();
        // A zero timeout would abandon every run instantly, which reads as the
        // feature being broken rather than as a configuration mistake.
        settings.run_timeout_secs = if config.run_timeout_secs == 0 {
            DEFAULT_RUN_TIMEOUT_SECS
        } else {
            config.run_timeout_secs
        };
        // Same reasoning as the timeout: a zero ceiling would make every agent
        // node wait forever on a permit that never exists, which reads as a
        // hang rather than as a configuration mistake.
        settings.max_parallel_agents = if config.max_parallel_agents == 0 {
            DEFAULT_MAX_PARALLEL_AGENTS
        } else {
            config.max_parallel_agents
        };
        settings
    }
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
