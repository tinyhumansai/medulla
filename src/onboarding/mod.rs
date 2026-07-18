//! First-run worker registration orchestration.
//!
//! [`ensure_registered`] is the single entry both `medulla daemon` and the
//! `medulla codex|claude|opencode` wrappers call before they start serving. It
//! decides whether onboarding is needed (no profile, or `--reonboard`), then
//! either drives the interactive [`OnboardingScreen`] on a TTY or auto-registers
//! headlessly, persists the [`WorkerProfile`], and — when an owner is set — sends
//! a one-time introduction DM.
//!
//! The pure state machine + rendering live in [`crate::ui::onboarding`]; the pure
//! profile model in [`crate::worker_profile`]. This module owns only the async
//! wiring (identity bootstrap, the pre-run terminal loop, the announce DM).

use std::collections::HashMap;
use std::io::{self, Stdout, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{Event, EventStream, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::tinyplace_support::{
    config_path, load_config, load_or_create_identity, resolve_endpoint, TinyPlaceConfig,
};
use crate::ui::onboarding::{OnboardingCmd, OnboardingEvent, OnboardingOutcome, OnboardingScreen};
use crate::worker_profile::{default_worker_name, is_registered, profile_path, WorkerProfile};
use tinyplace::auth::timestamp;
use tinyplace::{LocalSigner, Signer, TinyPlaceClient, TinyPlaceClientOptions};

/// The result of an onboarding check.
pub struct Registration {
    /// The active worker profile (loaded or freshly written).
    pub profile: WorkerProfile,
    /// True when this call ran onboarding and wrote a new profile.
    pub newly_registered: bool,
}

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
/// Returns `Ok(None)` only when the operator aborts the interactive flow (q /
/// Ctrl-C) — the caller should exit cleanly without serving. Otherwise a
/// [`Registration`] is returned (existing or freshly written).
pub async fn ensure_registered(
    env: &HashMap<String, String>,
    is_tty: bool,
    reonboard: bool,
) -> anyhow::Result<Option<Registration>> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let profile_file = profile_path(env, &home);
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

    let (name, owner) = if is_tty {
        match run_onboarding_screen(
            default_name,
            prefill_owner,
            endpoint.clone(),
            address.clone(),
            signer.clone(),
        )
        .await?
        {
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

/// The pre-run onboarding loop: set up a minimal terminal, drive the
/// [`OnboardingScreen`], and run its async command (a best-effort `@handle`
/// reverse-lookup) until it reaches an outcome. Returns the chosen
/// `(name, owner)` or `None` on abort.
async fn run_onboarding_screen(
    default_name: String,
    env_owner: Option<String>,
    endpoint: String,
    address: String,
    signer: Arc<LocalSigner>,
) -> anyhow::Result<Option<(String, Option<String>)>> {
    let mut guard = OnboardTermGuard::setup()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut screen = OnboardingScreen::new(default_name, env_owner, endpoint.clone());
    let mut reader = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(90));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<OnboardingEvent>();

    let outcome = loop {
        terminal.draw(|f| screen.draw(f))?;
        if let Some(outcome) = screen.outcome() {
            break outcome;
        }

        tokio::select! {
            maybe_event = reader.next() => {
                if let Some(Ok(Event::Key(key))) = maybe_event {
                    if key.kind != KeyEventKind::Release {
                        if let Some(cmd) = screen.handle_key(key) {
                            dispatch(cmd, &endpoint, &address, &signer, &tx);
                        }
                    }
                }
            }
            Some(ev) = rx.recv() => screen.apply(ev),
            _ = tick.tick() => screen.tick(),
        }
    };

    guard.restore();

    Ok(match outcome {
        OnboardingOutcome::Register { name, owner } => Some((name, owner)),
        OnboardingOutcome::Abort => None,
    })
}

/// Spawn the async work an [`OnboardingCmd`] requires. The only command is the
/// identity step: the address is already known, so this just does a best-effort
/// reverse `@handle` lookup (short-lived, failure → no handle) before emitting
/// [`OnboardingEvent::IdentityReady`].
fn dispatch(
    cmd: OnboardingCmd,
    endpoint: &str,
    address: &str,
    signer: &Arc<LocalSigner>,
    tx: &tokio::sync::mpsc::UnboundedSender<OnboardingEvent>,
) {
    match cmd {
        OnboardingCmd::LoadIdentity { .. } => {
            let endpoint = endpoint.to_string();
            let address = address.to_string();
            let signer = signer.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                let handle = lookup_handle(&endpoint, &address, signer).await;
                let _ = tx.send(OnboardingEvent::IdentityReady { address, handle });
            });
        }
    }
}

/// Best-effort reverse lookup of a `@handle` for `address` (2s budget). Any error
/// or timeout yields `None` — a worker without a claimed handle is normal.
async fn lookup_handle(endpoint: &str, address: &str, signer: Arc<LocalSigner>) -> Option<String> {
    let client = TinyPlaceClient::new(TinyPlaceClientOptions {
        base_url: endpoint.to_string(),
        signer: Some(signer as Arc<dyn Signer>),
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
    tp_config: &TinyPlaceConfig,
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

/// A minimal raw-mode + alt-screen guard for the pre-run onboarding loop. Unlike
/// the main TUI it needs no mouse capture or kitty flags — it is keyboard-only.
struct OnboardTermGuard {
    active: bool,
}

impl OnboardTermGuard {
    fn setup() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut out = io::stdout();
        execute!(out, EnterAlternateScreen)?;
        out.flush()?;
        Ok(OnboardTermGuard { active: true })
    }

    fn restore(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        let mut out: Stdout = io::stdout();
        let _ = execute!(out, LeaveAlternateScreen);
        let _ = disable_raw_mode();
        let _ = out.flush();
    }
}

impl Drop for OnboardTermGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn env_owner_priority_order() {
        assert_eq!(
            env_owner(&env(&[("TINYPLACE_OPENHUMAN_OWNER", "@boss")])).as_deref(),
            Some("@boss")
        );
        // Harness DM_TO wins over the generic owner.
        assert_eq!(
            env_owner(&env(&[
                ("TINYPLACE_HARNESS_DM_TO", "@dm"),
                ("TINYPLACE_OPENHUMAN_OWNER", "@boss"),
            ]))
            .as_deref(),
            Some("@dm")
        );
        // Legacy key is last.
        assert_eq!(
            env_owner(&env(&[("OPENHUMAN_OWNER_AGENT", "addr-1")])).as_deref(),
            Some("addr-1")
        );
        // Blank values are skipped.
        assert_eq!(
            env_owner(&env(&[
                ("TINYPLACE_HARNESS_DM_TO", "  "),
                ("TINYPLACE_OPENHUMAN_OWNER", "@boss"),
            ]))
            .as_deref(),
            Some("@boss")
        );
        assert_eq!(env_owner(&env(&[])), None);
    }

    #[test]
    fn identity_present_reads_env_and_config() {
        let dir = std::env::temp_dir().join(format!("medulla-onb-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let config_file = dir.join("config.json");

        // Nothing yet.
        assert!(!identity_present(&config_file, &env(&[])));
        // Env key present.
        assert!(identity_present(
            &config_file,
            &env(&[("TINYPLACE_SECRET_KEY", &"a".repeat(64))])
        ));
        // Blank env key is not "present".
        assert!(!identity_present(
            &config_file,
            &env(&[("TINYPLACE_SECRET_KEY", "  ")])
        ));

        // Config with a secret key.
        std::fs::write(&config_file, r#"{"secretKey":"deadbeef"}"#).unwrap();
        assert!(identity_present(&config_file, &env(&[])));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn headless_auto_registers_with_env_owner() {
        let dir =
            std::env::temp_dir().join(format!("medulla-onb-hl-{}-{}", std::process::id(), "a"));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = env(&[
            ("MEDULLA_HOME", dir.join("home").to_str().unwrap()),
            ("TINYPLACE_CONFIG", dir.join("tp.json").to_str().unwrap()),
            ("TINYPLACE_OPENHUMAN_OWNER", "@overseer"),
            ("USER", "ada"),
            ("HOSTNAME", "box-1"),
        ]);
        // Provide a fixed identity so no network is needed and the address is stable.
        let signer = LocalSigner::generate();
        let hex: String = signer.seed().iter().map(|b| format!("{b:02x}")).collect();
        e.insert("TINYPLACE_SECRET_KEY".to_string(), hex);

        // Not registered yet → headless path writes a profile.
        let reg = ensure_registered(&e, false, false)
            .await
            .unwrap()
            .expect("headless registers");
        assert!(reg.newly_registered);
        // <user>@<host>/<ip> — the ip segment is best-effort.
        assert!(
            reg.profile.name.starts_with("ada@box-1/"),
            "name: {}",
            reg.profile.name
        );
        assert_eq!(reg.profile.owner.as_deref(), Some("@overseer"));
        assert_eq!(reg.profile.address, signer.agent_id());
        assert!(reg.profile.registered_at.is_some());

        // Second call: already registered, returns it without re-writing.
        let again = ensure_registered(&e, false, false)
            .await
            .unwrap()
            .expect("still registered");
        assert!(!again.newly_registered);
        assert_eq!(again.profile.address, signer.agent_id());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn headless_without_owner_still_registers() {
        let dir =
            std::env::temp_dir().join(format!("medulla-onb-hl-{}-{}", std::process::id(), "b"));
        let _ = std::fs::remove_dir_all(&dir);
        let signer = LocalSigner::generate();
        let hex: String = signer.seed().iter().map(|b| format!("{b:02x}")).collect();
        let e = env(&[
            ("MEDULLA_HOME", dir.join("home").to_str().unwrap()),
            ("TINYPLACE_CONFIG", dir.join("tp.json").to_str().unwrap()),
            ("TINYPLACE_SECRET_KEY", &hex),
            ("USER", "grace"),
            ("HOSTNAME", "node"),
        ]);
        let reg = ensure_registered(&e, false, false)
            .await
            .unwrap()
            .expect("registers with no owner");
        assert!(reg.newly_registered);
        assert_eq!(reg.profile.owner, None);
        assert!(reg.profile.name.starts_with("grace@node/"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
