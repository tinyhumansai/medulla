//! First-run worker registration orchestration.
//!
//! [`ensure_registered`] is the single entry both `medulla daemon` and the
//! `medulla codex|claude|opencode` wrappers call before they start serving. It
//! decides whether onboarding is needed (no profile, or `--reonboard`), then
//! either drives an injected interactive UI (on a TTY) or auto-registers
//! headlessly, persists the [`WorkerProfile`], and — when an owner is set — sends
//! a one-time introduction DM.
//!
//! Interactivity is now dependency-inverted: the SDK owns no terminal
//! rendering. Callers pass an [`OnboardingUi`] callback (built by the app crate)
//! when they want the interactive screen; passing `None` runs headlessly. The
//! UI can call the public [`reverse_handle_lookup`] helper for a best-effort
//! `@handle`. The pure profile model lives in [`crate::worker_profile`].

mod types;

#[cfg(test)]
mod tests;

pub use types::{OnboardingContext, OnboardingUi, Registration};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::tinyplace::{
    config_path, load_config, load_or_create_identity, resolve_endpoint, TinyplaceFileConfig,
};
use crate::worker_profile::{default_worker_name, is_registered, profile_path, WorkerProfile};
use ::tinyplace::auth::timestamp;
use ::tinyplace::{LocalSigner, Signer, TinyPlaceClient, TinyPlaceClientOptions};

/// The OpenHuman owner from the generic environment chain, in priority order:
/// `TINYPLACE_HARNESS_DM_TO` → `TINYPLACE_OPENHUMAN_OWNER` → `OPENHUMAN_OWNER_AGENT`.
/// (The wrapper layers a per-provider `TINYPLACE_<P>_DM_TO` in front of this.)
pub fn env_owner(env: &HashMap<String, String>) -> Option<String> {
    for key in [
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

/// Whether a tiny.place identity is present without minting one:
/// `TINYPLACE_SECRET_KEY` in `env`, else a `secretKey` in the config file.
pub fn identity_present(config_file: &Path, env: &HashMap<String, String>) -> bool {
    if env
        .get("TINYPLACE_SECRET_KEY")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
    {
        return true;
    }
    load_config(config_file)
        .secret_key
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
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
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let profile_file = profile_path(env);
    let config_file = config_path(env, &home);
    let existing = WorkerProfile::load(&profile_file);

    if !reonboard && is_registered(existing.as_ref(), identity_present(&config_file, env)) {
        return Ok(Some(Registration {
            profile: existing.expect("registered implies a profile"),
            newly_registered: false,
        }));
    }

    // Onboarding needed: load or mint the identity now.
    let (signer, tp_config) =
        load_or_create_identity(&config_file, env).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let signer = Arc::new(signer);
    let address = signer.agent_id();
    let endpoint = resolve_endpoint(env, &tp_config);
    let default_name = existing
        .as_ref()
        .map(|p| p.name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| default_worker_name(env));
    let prefill_owner = env_owner(env)
        .or_else(|| tp_config.open_human_owner.clone())
        .or_else(|| existing.as_ref().and_then(|p| p.owner.clone()));

    let (name, owner) = if let Some(run) = ui {
        // Interactive: hand the app-provided UI everything it needs, then use
        // its chosen name/owner (or abort cleanly on `None`).
        let context = OnboardingContext {
            default_name,
            prefill_owner,
            endpoint,
            address: address.clone(),
            signer: signer.clone(),
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
                 (set $TINYPLACE_OPENHUMAN_OWNER or run with --reonboard on a TTY to set one)"
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
        registered_at: Some(timestamp()),
    };
    profile
        .save(&profile_file)
        .map_err(|e| anyhow::anyhow!("failed to write worker profile: {e}"))?;

    // Best-effort one-time introduction DM to the owner.
    if owner.is_some() {
        announce(env, &config_file, &home, &signer, &tp_config, &profile).await;
    }

    Ok(Some(Registration {
        profile,
        newly_registered: true,
    }))
}

/// Best-effort reverse lookup of a `@handle` for `address` (2s budget). Any error
/// or timeout yields `None` — a worker without a claimed handle is normal. This
/// is the SDK-side identity step an interactive [`OnboardingUi`] runs; it uses
/// the tiny.place client only, so it stays in the SDK.
pub async fn reverse_handle_lookup(
    endpoint: &str,
    address: &str,
    signer: &LocalSigner,
) -> Option<String> {
    let client = TinyPlaceClient::new(TinyPlaceClientOptions {
        base_url: endpoint.to_string(),
        signer: Some(Arc::new(signer.clone()) as Arc<dyn Signer>),
        ..Default::default()
    });
    let reverse = tokio::time::timeout(Duration::from_secs(2), client.directory.reverse(address))
        .await
        .ok()?
        .ok()?;
    let username = reverse.identities.into_iter().find_map(|id| {
        let name = id.username.trim().to_string();
        (!name.is_empty()).then_some(name)
    })?;
    Some(format!("@{username}"))
}

/// Send a one-time introduction DM to the profile owner. Best-effort: identity /
/// transport / send failures are logged and swallowed.
async fn announce(
    env: &HashMap<String, String>,
    config_file: &Path,
    home: &Path,
    signer: &Arc<LocalSigner>,
    tp_config: &TinyplaceFileConfig,
    profile: &WorkerProfile,
) {
    let Some(owner) = profile.owner.as_deref() else {
        return;
    };
    let base_url = resolve_endpoint(env, tp_config);
    let client = TinyPlaceClient::new(TinyPlaceClientOptions {
        base_url,
        signer: Some(signer.clone() as Arc<dyn Signer>),
        ..Default::default()
    });
    let identity_dir = config_file
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".tinyplace"));
    let transport =
        crate::daemon::transport::SignalTransport::new(client, signer.as_ref(), &identity_dir);
    if let Err(err) = transport.publish_keys(signer.as_ref()).await {
        eprintln!("medulla: onboarding pre-key publish failed ({err})");
    }
    let hello = format!(
        "👋 Worker \"{}\" is now registered to you and online via medulla.",
        profile.name
    );
    if let Err(err) = transport.send(owner, &hello).await {
        eprintln!("medulla: onboarding introduction DM to {owner} failed ({err})");
    }
}
