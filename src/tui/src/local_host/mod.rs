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

use medulla::bridge::{Bridge, LocalBridgeNetwork};
use medulla::config::HostSection;
use medulla::daemon::embedded::{EmbeddedDaemon, EmbeddedDaemonOptions};
use medulla::hub::WorkerSpec;
use medulla::tinyplace::HarnessProvider;
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
pub(crate) fn options_from_config(
    config: &HostSection,
    env: &HashMap<String, String>,
    router: Option<medulla::config::RouterConfig>,
    budget: Option<medulla::config::BudgetConfig>,
    log: Option<medulla::hub::HubLog>,
) -> Result<EmbeddedDaemonOptions, String> {
    let providers = config
        .providers
        .iter()
        .map(|name| parse_provider(name))
        .collect::<Result<Vec<_>, _>>()?;
    let default_provider = match config.default_provider.trim() {
        "" => None,
        name => Some(parse_provider(name)?),
    };
    Ok(EmbeddedDaemonOptions {
        workspace: config.workspace.clone(),
        providers: (!providers.is_empty()).then_some(providers),
        default_provider,
        concurrency: config.concurrency.max(1) as usize,
        task_timeout_ms: config.task_timeout_ms,
        model: (!config.model.trim().is_empty()).then(|| config.model.trim().to_string()),
        skip_permissions: config.skip_permissions,
        env: env.clone(),
        router,
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
    }
}

/// Start hosting on this device, or explain why not.
///
/// Returns `Ok(None)` when hosting is switched off, which is a choice rather
/// than a failure. An `Err` means hosting was wanted and could not happen — no
/// agent CLI installed, or the address already bound — and the caller surfaces
/// it, because an orchestrator with no host silently does nothing at all.
pub(crate) fn start(
    config: &HostSection,
    env: &HashMap<String, String>,
    network: &LocalBridgeNetwork,
    options: EmbeddedDaemonOptions,
) -> Result<Option<LocalHost>, String> {
    if !host_enabled(config, env) {
        return Ok(None);
    }
    let address = host_address(config);
    let bridge = network
        .bind(&address)
        .map_err(|e| format!("could not host on this device ({e})"))?;
    let daemon = EmbeddedDaemon::start(
        std::sync::Arc::new(bridge) as std::sync::Arc<dyn Bridge>,
        &address,
        options,
    )?;
    let spec = spec_for(&daemon);
    Ok(Some(LocalHost { daemon, spec }))
}
