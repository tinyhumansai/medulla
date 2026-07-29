//! Hosting tasks on the machine the TUI is running on.
//!
//! A plain `medulla` is both halves of the system: the orchestrator that decides
//! what work to hand out, and a host that runs it. Before this, only the first
//! half was in the process — the second needed a separate `medulla daemon`, a
//! tiny.place identity for each side, and a contact edge between two programs on
//! the same laptop, all so a task could travel through a relay to arrive back
//! where it started.
//!
//! Here the host binds an address on an in-memory bus that the hub also
//! dispatches over ([`RoutingBridge`](medulla::bridge::RoutingBridge)), so work
//! for this device is delivered in-process and everything else still goes to
//! remote workers over tiny.place. Configuration is the `[host]` section, with
//! `MEDULLA_HOST=0` as the single-run kill switch.
//!
//! Tasks run in **watchable** harness sessions
//! ([`PtySessionExecutor`](medulla_tui::worker::executor::PtySessionExecutor)),
//! the same executor the worker daemon's `--tui` mode uses. The headless
//! `-p --output-format stream-json` executor is no longer selected here: it
//! suppresses the very interface the Agents tab now renders, so choosing it
//! would leave that pane with nothing to show.

use medulla::bridge::{Bridge, LocalBridgeNetwork};
use medulla::config::HostSection;
use medulla::daemon::embedded::{resolve_workspace, EmbeddedDaemon, EmbeddedDaemonOptions};
use medulla::daemon::providers::{run_provider_task, RunTaskFn, RunTaskOptions};
use medulla::hub::WorkerSpec;
use medulla::tinyplace::HarnessProvider;
use medulla_tui::worker::executor::{agent_kind, PtySessionExecutor};
use medulla_tui::worker::pty::PtyManager;
use std::collections::HashMap;

mod types;

#[cfg(test)]
mod tests;

pub(crate) use types::LocalHost;

/// Whether this device should host tasks.
///
/// The config decides, and `MEDULLA_HOST` overrides it either way for one run —
/// the same shape as `MEDULLA_HUB`, so "turn the other half off" is one
/// consistent thing to remember rather than two.
pub(crate) fn host_enabled(config: &HostSection, env: &HashMap<String, String>) -> bool {
    match env
        .get("MEDULLA_HOST")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        Some(value) => medulla::home::is_truthy(value),
        None => config.enabled,
    }
}

/// The address this host binds on the local bus.
///
/// Falls back to the section default when the operator blanked it, because an
/// empty address cannot be bound and silently not hosting is worse than hosting
/// under the documented name.
pub(crate) fn host_address(config: &HostSection) -> String {
    match config.address.trim() {
        "" => HostSection::default().address,
        value => value.to_string(),
    }
}

/// Every device-local address a host could bind, running or not.
///
/// Known without starting anything, because it comes from the config rather
/// than from a started host — and it is needed in exactly the case where none
/// started, to recognise remembered local roster entries and drop them.
pub(crate) fn all_host_addresses(primary: &HostSection, extras: &[HostSection]) -> Vec<String> {
    std::iter::once(host_address(primary))
        .chain(
            extras
                .iter()
                .enumerate()
                .map(|(index, extra)| extra_host_address(extra, index)),
        )
        .collect()
}

/// The bus address for an extra host, derived from its name when it declared
/// none of its own.
///
/// Two hosts cannot share an address — the second `bind` fails — so an operator
/// who adds `[[hosts]]` without thinking about addressing would otherwise get
/// one working host and one startup error. Deriving from the name means the
/// field is optional in the common case and explicit when it matters.
fn extra_host_address(config: &HostSection, fallback_index: usize) -> String {
    // The section default counts as unchosen, not as a choice. `[[hosts]]`
    // shares `HostSection`, so an entry that names no address inherits the
    // primary's — and two hosts on one address means the second never binds.
    // An operator who *typed* the primary's address has made the same mistake,
    // so both are treated the same way.
    let chosen = config.address.trim();
    let chosen = if chosen == HostSection::default().address {
        ""
    } else {
        chosen
    };
    match chosen {
        "" => {
            let slug = slug_of(&config.name);
            if slug.is_empty() {
                format!("local-host-{}", fallback_index + 1)
            } else {
                format!("local-{slug}")
            }
        }
        value => value.to_string(),
    }
}

/// A lowercase, hyphenated form of `name`, safe to use as a bus address.
fn slug_of(name: &str) -> String {
    let mut out = String::new();
    let mut hyphen = false;
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            hyphen = false;
        } else if !out.is_empty() && !hyphen {
            out.push('-');
            hyphen = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

/// What to call a host that named itself nothing.
///
/// The primary is "this device" — it is the machine the operator is looking at.
/// An extra is named for the directory it works in, because that is the only
/// thing distinguishing it from the primary.
pub(crate) fn display_name(config: &HostSection, workspace: &str, primary: bool) -> String {
    match config.name.trim() {
        "" if primary => "this device".to_string(),
        "" => std::path::Path::new(workspace)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| workspace.to_string()),
        value => value.to_string(),
    }
}

/// Translate the `[host]` section into the SDK's start-up options.
///
/// # Errors
///
/// An unrecognized provider name is rejected rather than skipped. Dropping one
/// silently inverts the operator's intent: `providers = ["claudde"]` would parse
/// to an empty list, and an empty list means *detect everything installed* — so
/// a typo meant to narrow what this machine runs would instead widen it, and
/// unattended work would go to a CLI nobody chose, with permission prompts
/// bypassed. The same applies to `defaultProvider`, where an unknown name would
/// quietly fall back to whichever CLI happened to be detected first.
#[cfg(test)]
pub(crate) fn options_from_config(
    config: &HostSection,
    env: &HashMap<String, String>,
    router: Option<medulla::config::RouterConfig>,
    budget: Option<medulla::config::BudgetConfig>,
    log: Option<medulla::hub::HubLog>,
) -> Result<EmbeddedDaemonOptions, String> {
    options_from_config_with_custom(config, env, router, budget, log, &[])
}

/// Translate host config and named custom-harness presets into start-up options.
///
/// Presets attach by `hostId`; a preset for another fleet machine is not
/// advertised or executable on this device. Its base CLI is added to an
/// explicit provider allowlist because declaring a custom harness is itself an
/// explicit request to run that CLI.
pub(crate) fn options_from_config_with_custom(
    config: &HostSection,
    env: &HashMap<String, String>,
    router: Option<medulla::config::RouterConfig>,
    budget: Option<medulla::config::BudgetConfig>,
    log: Option<medulla::hub::HubLog>,
    custom_harnesses: &[medulla::config::CustomHarnessConfig],
) -> Result<EmbeddedDaemonOptions, String> {
    let address = host_address(config);
    let mut providers = config
        .providers
        .iter()
        .map(|name| parse_provider(name))
        .collect::<Result<Vec<_>, _>>()?;
    let custom_harnesses: Vec<_> = custom_harnesses
        .iter()
        .filter(|harness| harness.host_id == address)
        .cloned()
        .collect();
    for provider in custom_harnesses.iter().map(|harness| harness.base_harness) {
        if !providers.contains(&provider) {
            providers.push(provider);
        }
    }
    let custom_default = custom_harnesses
        .iter()
        .find(|harness| harness.default)
        .map(|harness| harness.base_harness);
    let default_provider = match config.default_provider.trim() {
        "" => custom_default,
        name => Some(parse_provider(name)?),
    };
    Ok(EmbeddedDaemonOptions {
        workspace: config.workspace.clone(),
        workspaces: config.workspaces.clone(),
        providers: (!providers.is_empty()).then_some(providers),
        default_provider,
        concurrency: config.concurrency.max(1) as usize,
        task_timeout_ms: config.task_timeout_ms,
        model: (!config.model.trim().is_empty()).then(|| config.model.trim().to_string()),
        skip_permissions: config.skip_permissions,
        env: env.clone(),
        router,
        custom_harnesses,
        budget,
        log,
        ..Default::default()
    })
}

/// Parse one configured provider name, naming the valid spellings on failure.
fn parse_provider(name: &str) -> Result<HarnessProvider, String> {
    HarnessProvider::from_wire(name.trim()).ok_or_else(|| {
        format!(
            "unknown harness \"{}\" in [host] — expected one of: claude, codex, opencode",
            name.trim()
        )
    })
}

/// The roster entry describing a host running on this machine.
///
/// Labelled rather than left to the default name so the fleet view can say "this
/// device" instead of showing a bare address the operator never chose. Extra
/// hosts get their own name: several hosts on one machine differ only by where
/// they work, so "this device" three times would describe none of them.
fn spec_for(daemon: &EmbeddedDaemon, name: &str) -> WorkerSpec {
    let harness = daemon.default_provider().as_str().to_string();
    WorkerSpec {
        id: daemon.address().to_string(),
        address: daemon.address().to_string(),
        name: name.to_string(),
        description: format!(
            "{} on this machine · {}",
            daemon
                .providers()
                .iter()
                .map(|provider| provider.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            daemon.workspace()
        ),
        harness,
        // The one roster entry whose workspace this process actually knows: the
        // host runs in this directory. Declaring it is what gives the
        // orchestrator a placed agent rather than a bare one it treats as
        // having nowhere to work.
        workspace: Some(daemon.workspace().to_string()),
    }
}

/// Start hosting on this device, or explain why not.
///
/// Returns `Ok(None)` when hosting is switched off, which is a choice rather
/// than a failure. An `Err` means hosting was wanted and could not happen — no
/// agent CLI installed, or the address already bound — and the caller surfaces
/// it, because an orchestrator with no host silently does nothing at all.
///
/// Tasks run in **live harness sessions** on `sessions`, not in headless
/// one-shots — for the providers that write a transcript this can tail.
/// `claude`/`codex` are started the way a human starts them — no `-p`, no
/// `--output-format` — so they paint their own interface, and the Agents tab
/// renders that interface instead of a transcript reconstructed from JSON. The
/// manager is passed in rather than built here because the UI needs the same
/// one to read screens from and type into.
///
/// OpenCode is the one provider this does not cover
/// ([`agent_kind`] returns `None` for it: no flat transcript to tail, so no way
/// to know a turn ended) and [`run_task`] routes it to the headless executor
/// instead, exactly as `EmbeddedDaemon::start` would have. Detection still
/// advertises whatever is installed regardless of which executor serves it —
/// an OpenCode-only machine must still start and actually complete tasks, not
/// merely start and then fail every one against `PtySessionExecutor`'s refusal.
pub(crate) fn start(
    config: &HostSection,
    env: &HashMap<String, String>,
    network: &LocalBridgeNetwork,
    options: EmbeddedDaemonOptions,
    sessions: PtyManager,
) -> Result<Option<LocalHost>, String> {
    if !host_enabled(config, env) {
        return Ok(None);
    }
    start_at(
        config,
        env,
        network,
        options,
        sessions,
        host_address(config),
        true,
    )
    .map(Some)
}

/// Starts a host on this device after the app is already running.
///
/// Everything a host needs to exist — the in-process bus, the session manager,
/// the daemon options — is built once at launch and owned by the app loop. A
/// host declared later has no way to reach any of it, which is why adding one
/// used to mean restarting. This carries exactly those pieces to wherever the
/// command is handled.
///
/// Cheap to clone: every field is already shared.
#[derive(Clone)]
pub(crate) struct LocalHostSpawner {
    /// The bus the hub dispatches over.
    network: LocalBridgeNetwork,
    /// The session manager the UI reads screens from and types into. Shared, so
    /// a host started now is as watchable as one started at launch.
    sessions: PtyManager,
    /// The primary's options, used as the template every extra inherits.
    options: EmbeddedDaemonOptions,
    /// The process environment, for provider detection and the host switch.
    env: HashMap<String, String>,
    /// The runtimes the harness pane resolves tasks against. A new host's
    /// runtime is pushed here or its screen would never be found.
    runtimes: std::sync::Arc<std::sync::Mutex<Vec<medulla::daemon::DaemonRuntime>>>,
    /// The started hosts, kept alive for the session. Dropping a `LocalHost`
    /// stops it, so a spawner that did not hold them would start a host and
    /// immediately kill it.
    started: std::sync::Arc<std::sync::Mutex<Vec<LocalHost>>>,
}

impl LocalHostSpawner {
    /// Build a spawner over the pieces the app loop owns.
    pub(crate) fn new(
        network: LocalBridgeNetwork,
        sessions: PtyManager,
        options: EmbeddedDaemonOptions,
        env: HashMap<String, String>,
        runtimes: std::sync::Arc<std::sync::Mutex<Vec<medulla::daemon::DaemonRuntime>>>,
        started: std::sync::Arc<std::sync::Mutex<Vec<LocalHost>>>,
    ) -> Self {
        Self {
            network,
            sessions,
            options,
            env,
            runtimes,
            started,
        }
    }

    /// Start `config` now and return the roster entry describing it.
    ///
    /// The address is derived from the count of hosts already started, so a
    /// second unnamed host does not collide with the first.
    pub(crate) fn spawn(&self, config: &HostSection) -> Result<WorkerSpec, String> {
        let index = self.started.lock().expect("started hosts").len();
        let mut options = self.options.clone();
        options.workspace = config.workspace.clone();
        let host = start_at(
            config,
            &self.env,
            &self.network,
            options,
            self.sessions.clone(),
            extra_host_address(config, index),
            false,
        )?;
        let spec = host.spec().clone();
        self.runtimes
            .lock()
            .expect("local harness runtimes")
            .push(host.runtime());
        self.started.lock().expect("started hosts").push(host);
        Ok(spec)
    }
}

/// Start every host this machine declares: the `[host]` primary, then each
/// `[[hosts]]` entry.
///
/// One process, several working directories. Each entry binds its own bus
/// address and registers as its own agent, so the orchestrator can be told which
/// *project* to work in rather than only which machine — and because they share
/// this process's session manager, every one of them stays readable and typeable
/// in the Agents pane, which is what spawning separate daemons costs you.
///
/// A failing extra does not take the others down. An operator who mistypes one
/// directory should lose that host, not hosting altogether, so the error is
/// returned alongside the hosts that did start and the caller reports it.
pub(crate) fn start_all(
    primary: &HostSection,
    extras: &[HostSection],
    env: &HashMap<String, String>,
    network: &LocalBridgeNetwork,
    options: EmbeddedDaemonOptions,
    sessions: PtyManager,
) -> (Vec<LocalHost>, Vec<String>) {
    let mut hosts = Vec::new();
    let mut problems = Vec::new();

    match start(primary, env, network, options.clone(), sessions.clone()) {
        Ok(Some(host)) => hosts.push(host),
        Ok(None) => {}
        Err(error) => problems.push(error),
    }

    for (index, extra) in extras.iter().enumerate() {
        if !host_enabled(extra, env) {
            continue;
        }
        // Each extra overrides only what makes it distinct — where it works —
        // and inherits the rest of the primary's options, so declaring one is a
        // directory and nothing else.
        let mut extra_options = options.clone();
        extra_options.workspace = extra.workspace.clone();
        let address = extra_host_address(extra, index);
        match start_at(
            extra,
            env,
            network,
            extra_options,
            sessions.clone(),
            address,
            false,
        ) {
            Ok(host) => hosts.push(host),
            Err(error) => problems.push(error),
        }
    }
    (hosts, problems)
}

/// Bind one host at `address` and wrap it in its roster entry.
fn start_at(
    config: &HostSection,
    env: &HashMap<String, String>,
    network: &LocalBridgeNetwork,
    options: EmbeddedDaemonOptions,
    sessions: PtyManager,
    address: String,
    primary: bool,
) -> Result<LocalHost, String> {
    let bridge = network
        .bind(&address)
        .map_err(|e| format!("could not host on this device ({e})"))?;
    // The executor is built before the host, so it cannot ask the host where
    // tasks will run; it resolves the same configured workspace the host is
    // about to resolve. The two must agree — this is the directory the session
    // tailer searches for the harness's transcript.
    let executor =
        PtySessionExecutor::new(sessions, env.clone(), resolve_workspace(&options.workspace));
    let daemon = EmbeddedDaemon::start_with_executor(
        std::sync::Arc::new(bridge) as std::sync::Arc<dyn Bridge>,
        &address,
        options,
        run_task(executor),
    )?;
    let name = display_name(config, daemon.workspace(), primary);
    let spec = spec_for(&daemon, &name);
    Ok(LocalHost { daemon, spec })
}

/// Route each task by what it can actually run: [`PtySessionExecutor`] for a
/// provider it can tail, the headless one-shot executor for the one it
/// cannot.
///
/// A single provider check per task rather than two separate host paths —
/// dispatching per-task, not per-host, is what keeps a mixed installation
/// (say, claude *and* opencode both detected) fully usable: the claude lane
/// gets the watchable pane, the opencode lane still completes its work.
fn run_task(pty: PtySessionExecutor) -> RunTaskFn {
    let watchable = pty.into_run_task();
    std::sync::Arc::new(move |options: RunTaskOptions| {
        // `agent_kind` is the authority, not a provider allow-list: it answers
        // "is there a transcript this can tail", which is the actual
        // precondition `PtySessionExecutor` fails on. A list would silently
        // stop matching the moment a provider gained or lost one.
        if agent_kind(options.provider).is_some() {
            watchable(options)
        } else {
            Box::pin(run_provider_task(options))
        }
    })
}
