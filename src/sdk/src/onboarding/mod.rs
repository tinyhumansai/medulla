//! First-run worker registration orchestration.
//!
//! [`ensure_registered`] is the single entry both `medulla daemon` and the
//! `medulla codex|claude|opencode` wrappers call before they start serving. It
//! decides whether onboarding is needed (no profile, or `--reonboard`), then
//! either drives an injected interactive UI (on a TTY) or auto-registers
//! headlessly and persists the [`WorkerProfile`].
//!
//! Interactivity is now dependency-inverted: the SDK owns no terminal
//! rendering. Callers pass an [`OnboardingUi`] callback (built by the app crate)
//! when they want the interactive screen; passing `None` runs headlessly. The
//! The pure profile model lives in [`crate::worker_profile`].

mod types;

#[cfg(test)]
mod tests;

pub use types::{OnboardingContext, OnboardingUi, Registration};

use std::collections::HashMap;
use std::path::Path;

use crate::clock::iso_now;
use crate::worker_profile::{default_worker_name, is_registered, profile_path, WorkerProfile};

/// The owning orchestrator from the generic environment chain, in priority
/// order: `MEDULLA_LINK_OWNER` → `TINYPLACE_HARNESS_DM_TO` →
/// `TINYPLACE_OPENHUMAN_OWNER` → `OPENHUMAN_OWNER_AGENT`.
///
/// The three legacy names are kept behind the new one so a host that was
/// configured before the link existed keeps working: an env var lives in
/// someone's shell profile, and silently ignoring it would look like a worker
/// that forgot who it belongs to.
/// (The wrapper layers a per-provider `TINYPLACE_<P>_DM_TO` in front of this.)
pub fn env_owner(env: &HashMap<String, String>) -> Option<String> {
    for key in [
        "MEDULLA_LINK_OWNER",
        "TINYPLACE_HARNESS_DM_TO",
        "TINYPLACE_OPENHUMAN_OWNER",
        "OPENHUMAN_OWNER_AGENT",
    ] {
        if let Some(value) = env.get(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Whether this host holds a link identity — i.e. it has enrolled.
///
/// A read, never a mint: enrollment needs an invite token from the orchestrator
/// and a pair key typed in by hand (protocol §7), so nothing here can create one
/// on the worker's behalf.
pub fn identity_present(link_dir: &Path) -> bool {
    medulla_link::keys::node_path(link_dir).exists()
}

/// Ensure this worker is registered, running onboarding when needed.
///
/// When onboarding is required and `ui` is `Some(run)`, the interactive callback
/// drives the naming/owner flow and its `(name, owner)` result is used; returning
/// `Ok(None)` when the operator aborts (q / Ctrl-C). When `ui` is `None`, the
/// worker auto-registers headlessly with defaults + an env owner (if any) so the
/// daemon/wrapper stays scriptable. Otherwise a [`Registration`] is returned
/// (existing or freshly written).
pub async fn ensure_registered(
    env: &HashMap<String, String>,
    reonboard: bool,
    ui: Option<OnboardingUi>,
) -> anyhow::Result<Option<Registration>> {
    ensure_registered_in(env, reonboard, ui, Path::new(".")).await
}

/// Ensure this worker is registered using `config_cwd` for project config
/// discovery. Daemons pass their selected workspace so onboarding and serving
/// resolve the same link identity.
pub async fn ensure_registered_in(
    env: &HashMap<String, String>,
    reonboard: bool,
    ui: Option<OnboardingUi>,
    config_cwd: &Path,
) -> anyhow::Result<Option<Registration>> {
    let home = crate::home::medulla_home(env);
    let profile_file = profile_path(env);
    // Load the effective configuration to honor the configured link.stateDir if set.
    let explicit_config = crate::config::explicit_config_from_env(env);
    if let Some(path) = explicit_config {
        if !Path::new(path).is_file() {
            anyhow::bail!("explicit configuration does not exist: {path}");
        }
    }
    let link_dir = match crate::config::load_config(explicit_config, env, config_cwd) {
        Ok(loaded) => loaded
            .config
            .link
            .map(|cfg| cfg.state_dir.into())
            .unwrap_or_else(|| medulla_link::keys::link_dir(&home)),
        Err(err) if explicit_config.is_some() => {
            return Err(anyhow::anyhow!(
                "explicit configuration failed to load before onboarding: {err}"
            ));
        }
        Err(_) => medulla_link::keys::link_dir(&home),
    };
    let mut existing = WorkerProfile::load(&profile_file);
    if let (Some(profile), Ok(state)) = (
        existing
            .as_mut()
            .filter(|profile| profile.address.is_empty()),
        medulla_link::keys::read_node_state(&medulla_link::keys::node_path(&link_dir)),
    ) {
        profile.address = state.node_id.to_string();
        profile.save(&profile_file)?;
    }

    if !reonboard && is_registered(existing.as_ref(), identity_present(&link_dir)) {
        return Ok(Some(Registration {
            profile: existing.expect("registered implies a profile"),
            newly_registered: false,
        }));
    }

    // The node name and forwarder are only known once the host has enrolled; an
    // unenrolled worker still onboards (it gets a name and an owner) and simply
    // shows nothing for them.
    let enrolled =
        medulla_link::keys::read_node_state(&medulla_link::keys::node_path(&link_dir)).ok();
    let address = enrolled
        .as_ref()
        .map(|state| state.node_id.to_string())
        .unwrap_or_default();
    let endpoint = enrolled
        .as_ref()
        .map(|state| state.forwarder_endpoint.clone())
        .unwrap_or_default();
    let default_name = existing
        .as_ref()
        .map(|p| p.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| default_worker_name(env));
    let prefill_owner = env_owner(env).or_else(|| existing.as_ref().and_then(|p| p.owner.clone()));

    let (name, owner) = if let Some(run) = ui {
        // Interactive: hand the app-provided UI everything it needs, then use
        // its chosen name/owner (or abort cleanly on `None`).
        let context = OnboardingContext {
            default_name,
            prefill_owner,
            endpoint,
            address: address.clone(),
        };
        match run(context).await? {
            Some(pair) => pair,
            None => return Ok(None), // aborted
        }
    } else {
        // Headless: auto-register with defaults + env owner (if any) so the
        // daemon stays scriptable.
        if prefill_owner.is_none() {
            eprintln!(
                "medulla: registering worker \"{default_name}\" headlessly with no owner \
                 (set $MEDULLA_LINK_OWNER or run with --reonboard on a TTY to set one)"
            );
        } else {
            eprintln!("medulla: registering worker \"{default_name}\" headlessly");
        }
        (default_name, prefill_owner)
    };

    let profile = WorkerProfile {
        name,
        address,
        owner: owner.clone(),
        registered_at: Some(iso_now()),
    };
    profile
        .save(&profile_file)
        .map_err(|e| anyhow::anyhow!("failed to write worker profile: {e}"))?;

    Ok(Some(Registration {
        profile,
        newly_registered: true,
    }))
}
