//! The embedded host: a [`DaemonRuntime`] driven over any
//! [`Bridge`], inside someone else's process.
//!
//! [`super::entry::run_daemon`] is the standalone `medulla daemon` — it owns a
//! tiny.place identity, publishes pre-keys, registers a directory card, and
//! blocks until a signal. None of that is wanted when the orchestrator TUI wants
//! to *be* a host as well: it already has a process, and the peer it serves is
//! itself. [`EmbeddedDaemon::start`] is that path — hand it a bridge address and
//! it drains, dispatches, and replies exactly like the standalone daemon does,
//! with the transport left entirely to the caller.
//!
//! The result is that a plain `medulla` run is both orchestrator and host: the
//! hub dispatches to a device-local address, this module executes it against the
//! agent CLIs on this machine, and the reply comes back over the in-memory bus.
//! Pointing the same host at a tiny.place bridge instead is a caller-side
//! choice, not a different code path.

use std::sync::{Arc, Mutex};

use crate::bridge::Bridge;
use crate::protocol::{decode_task_frame, TaskFrameKind};

use super::providers::{detect_providers, run_provider_task, RunTaskFn, RunTaskOptions};
use super::types::{DaemonConfig, DaemonRuntime, SendFn};

mod types;

#[cfg(test)]
mod tests;

pub use types::{
    EmbeddedDaemon, EmbeddedDaemonOptions, EmbeddedDaemonStats, HostObservation,
    DEFAULT_CONCURRENCY, DEFAULT_LOCAL_POLL, DEFAULT_TASK_TIMEOUT_MS,
};

/// How many frames one drain pass takes. Bounded so a large backlog is handled
/// across several passes rather than admitted in one burst that ignores the
/// concurrency budget's back-pressure.
const DRAIN_LIMIT: i64 = 50;

impl EmbeddedDaemon {
    /// Start hosting tasks addressed to `bridge` on this machine.
    ///
    /// `bridge` must already be bound to the address peers reach this host at;
    /// `address` is that address, recorded so an orchestrator can put it on a
    /// roster. The drain loop is spawned before returning, so the host is live
    /// the moment this call succeeds.
    ///
    /// # Errors
    ///
    /// Fails when no coding-agent CLI is installed (or none of the requested
    /// ones), or when the requested default provider is not among them. Both are
    /// configuration mistakes an operator has to see rather than a host that
    /// starts and then rejects every task.
    pub fn start(
        bridge: Arc<dyn Bridge>,
        address: impl Into<String>,
        options: EmbeddedDaemonOptions,
    ) -> Result<Self, String> {
        let run_task: RunTaskFn =
            Arc::new(|options: RunTaskOptions| Box::pin(run_provider_task(options)));
        Self::start_with_executor(bridge, address, options, run_task)
    }

    /// Like [`start`](Self::start) with the task executor supplied by the caller.
    ///
    /// The executor is the seam between "which agent CLI runs this" and
    /// everything else the host does — draining, dispatch, correlation, replies.
    /// Overriding it lets a test drive the whole state machine without spawning a
    /// real process, and lets an embedder route tasks somewhere other than a
    /// local CLI. Provider *detection* still applies: the host advertises what is
    /// installed, so a substituted executor does not let it claim capabilities
    /// this machine does not have.
    pub fn start_with_executor(
        bridge: Arc<dyn Bridge>,
        address: impl Into<String>,
        options: EmbeddedDaemonOptions,
        run_task: RunTaskFn,
    ) -> Result<Self, String> {
        let address = address.into();
        let env = if options.env.is_empty() {
            std::env::vars().collect()
        } else {
            options.env.clone()
        };

        let providers = detect_providers(&env, options.providers.as_deref(), None);
        if providers.is_empty() {
            return Err(match &options.providers {
                Some(requested) => format!(
                    "none of the requested coding-agent CLIs are installed: {}",
                    requested
                        .iter()
                        .map(|p| p.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                None => "no coding-agent CLI found on PATH — install claude, codex, or opencode"
                    .to_string(),
            });
        }
        let default_provider = match options.default_provider {
            Some(provider) if !providers.contains(&provider) => {
                return Err(format!(
                    "default provider \"{}\" is not installed; found: {}",
                    provider.as_str(),
                    providers
                        .iter()
                        .map(|p| p.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            Some(provider) => provider,
            None => providers[0],
        };

        let workspace = resolve_workspace(&options.workspace);
        let stats = Arc::new(Mutex::new(EmbeddedDaemonStats::default()));

        let config = DaemonConfig {
            providers: providers.clone(),
            default_provider,
            workspace: workspace.clone(),
            accessible_dirs: accessible_dirs(&workspace, &options.workspaces),
            env,
            task_timeout_ms: options.task_timeout_ms,
            capability_timeout_ms: None,
            concurrency: options.concurrency.max(1),
            status_throttle_ms: super::types::DEFAULT_STATUS_THROTTLE_MS,
            max_pending: super::types::DEFAULT_MAX_PENDING,
            model: options.model.clone(),
            agent: options.agent.clone(),
            extra_args: Vec::new(),
            skip_permissions: options.skip_permissions,
            router: options.router.clone(),
            attribution: options.attribution,
            hooks: options.hooks.clone(),
            custom_harnesses: options.custom_harnesses.clone(),
            budget: options.budget.clone(),
        };

        let mut runtime = DaemonRuntime::new(config, run_task, observing_send(&bridge, &stats));
        if let Some(log) = options.log.clone() {
            runtime = runtime.with_log(log);
        }

        let drain = tokio::spawn(drain_loop(
            bridge,
            runtime.clone(),
            stats.clone(),
            options.poll,
        ));

        Ok(EmbeddedDaemon {
            address,
            workspace,
            providers,
            default_provider,
            runtime,
            stats,
            drain,
        })
    }

    /// The address peers dispatch to this host at.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// The absolute working directory tasks run in.
    pub fn workspace(&self) -> &str {
        &self.workspace
    }

    /// The coding-agent CLIs this host actually found and serves.
    pub fn providers(&self) -> &[crate::protocol::HarnessProvider] {
        &self.providers
    }

    /// The provider used when a task frame names none.
    pub fn default_provider(&self) -> crate::protocol::HarnessProvider {
        self.default_provider
    }

    /// A snapshot of what this host has been doing.
    pub fn stats(&self) -> EmbeddedDaemonStats {
        self.stats.lock().expect("embedded daemon stats").clone()
    }

    /// A cloneable read-only view, for a UI that renders this host from
    /// somewhere the host itself cannot be moved to.
    pub fn observation(&self) -> HostObservation {
        HostObservation::new(
            self.address.clone(),
            self.workspace.clone(),
            self.providers.clone(),
            self.default_provider,
            self.stats.clone(),
        )
    }

    /// The underlying task state machine, for a host that needs to reach past
    /// this wrapper (capability probes, workspace allowlist changes).
    pub fn runtime(&self) -> &DaemonRuntime {
        &self.runtime
    }

    /// Wait until every dispatched message has settled. Used by tests and by a
    /// host that wants a clean shutdown rather than an abrupt one.
    pub async fn idle(&self) {
        self.runtime.idle().await;
    }
}

/// Resolve the configured workspace to an absolute path.
///
/// An unresolvable path is kept verbatim rather than rejected: the daemon
/// reports a bad working directory per task, which names the failing task, and
/// that is more useful than refusing to start the host at all.
///
/// Public because a caller supplying its own executor via
/// [`EmbeddedDaemon::start_with_executor`] has a chicken-and-egg problem: the
/// executor is built *before* the host, but a session-backed one needs the same
/// resolved workspace the host will run tasks in. Exposing the resolution is
/// what keeps the two from disagreeing — the alternative is every embedder
/// reimplementing this and canonicalizing slightly differently.
pub fn resolve_workspace(configured: &str) -> String {
    let path = match configured.trim() {
        "" => std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        value => std::path::PathBuf::from(value),
    };
    std::fs::canonicalize(&path)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// The directories this host advertises, primary first.
///
/// The primary workspace always leads: it is where tasks actually run, and an
/// orchestrator reading the list top-down should see the default before the
/// alternatives. Each extra is canonicalized so `~/../repo` and `/Users/me/repo`
/// do not both appear, blanks are dropped, and duplicates collapse — a list that
/// names one directory twice reads as two places to work.
fn accessible_dirs(workspace: &str, extra: &[String]) -> Vec<String> {
    let mut out = vec![workspace.to_string()];
    for dir in extra {
        // Checked *before* resolving: `resolve_workspace("")` yields the process
        // cwd, but a blank config entry means a stray line, not "the directory I
        // launched from". Resolving first would silently advertise it.
        if dir.trim().is_empty() {
            continue;
        }
        let dir = resolve_workspace(dir);
        if !out.contains(&dir) {
            out.push(dir);
        }
    }
    out
}

/// Build the runtime's send callback, recording outbound frames on the way out.
///
/// Observation is done here rather than inside [`DaemonRuntime`] because every
/// terminal outcome the host produces leaves through this one callback — so the
/// counters cannot drift from what a peer actually saw.
fn observing_send(bridge: &Arc<dyn Bridge>, stats: &types::SharedStats) -> SendFn {
    let bridge = bridge.clone();
    let stats = stats.clone();
    Arc::new(move |to: String, body: String| {
        let bridge = bridge.clone();
        let stats = stats.clone();
        Box::pin(async move {
            record_outbound(&stats, &to, &body);
            if let Err(err) = bridge.send(&to, &body).await {
                eprintln!("medulla host: send to {to} failed: {err}");
            }
        })
    })
}

/// Fold one outbound frame into the observation counters.
fn record_outbound(stats: &types::SharedStats, to: &str, body: &str) {
    let Ok(mut stats) = stats.lock() else {
        return;
    };
    stats.last_peer = Some(to.to_string());
    let Some(frame) = decode_task_frame(body) else {
        return;
    };
    match frame.kind {
        // An ack is the host committing to run the task — the first moment it is
        // truthfully "started" from the peer's point of view.
        TaskFrameKind::Ack => stats.tasks_started += 1,
        TaskFrameKind::Reply => stats.tasks_completed += 1,
        TaskFrameKind::Error => stats.tasks_failed += 1,
        TaskFrameKind::Status => stats.last_status = Some(frame.text.clone()),
        TaskFrameKind::CapabilitiesResult | TaskFrameKind::SystemInfoResult => {
            stats.probes_answered += 1
        }
        _ => {}
    }
}

/// Drain the bridge and dispatch every inbound frame until aborted.
async fn drain_loop(
    bridge: Arc<dyn Bridge>,
    runtime: DaemonRuntime,
    stats: types::SharedStats,
    poll: std::time::Duration,
) {
    loop {
        tokio::time::sleep(poll).await;
        for message in bridge.drain_inbox(DRAIN_LIMIT).await {
            if let Ok(mut stats) = stats.lock() {
                stats.received += 1;
            }
            let frame = decode_task_frame(&message.text);
            runtime.handle_message(message.from, message.text, frame);
        }
    }
}
