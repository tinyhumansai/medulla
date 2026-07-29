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
    let mut providers = config
        .providers
        .iter()
        .map(|name| parse_provider(name))
        .collect::<Result<Vec<_>, _>>()?;
    let custom_harnesses: Vec<_> = custom_harnesses
        .iter()
        .filter(|harness| harness.host_id == config.address)
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
/// device" instead of showing a bare address the operator never chose.
fn spec_for(daemon: &EmbeddedDaemon) -> WorkerSpec {
    let harness = daemon.default_provider().as_str().to_string();
    WorkerSpec {
        id: daemon.address().to_string(),
        address: daemon.address().to_string(),
        name: "this device".to_string(),
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
    let address = host_address(config);
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
    let spec = spec_for(&daemon);
    Ok(Some(LocalHost { daemon, spec }))
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
