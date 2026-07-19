//! The persisted first-run worker profile.
//!
//! When a worker (the `medulla daemon` or a `medulla codex|claude|opencode`
//! wrapper) runs for the first time, onboarding names it and connects it to an
//! owner. The result is persisted as a small JSON document at
//! `<medulla-home>/worker.json` so subsequent launches skip the flow. The crate
//! has no `toml` dependency, so the profile is JSON (the field names are
//! camelCase to match the rest of the persisted state).
//!
//! "Registered" means both a profile file *and* a tiny.place identity exist. This
//! module only models and persists the profile; identity bootstrap lives in
//! [`crate::tinyplace::runtime`].

use std::collections::HashMap;
use std::io;
use std::net::UdpSocket;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The persisted worker identity: what the operator named this worker, its
/// tiny.place wallet address, the OpenHuman owner it answers to, and when it was
/// first registered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WorkerProfile {
    /// Operator-chosen worker name (defaults to [`default_worker_name`]).
    pub name: String,
    /// The tiny.place identity (wallet) address this worker registered with.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub address: String,
    /// The OpenHuman owner (`@handle` or address) this worker answers to.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub owner: Option<String>,
    /// ISO-8601 timestamp of first registration.
    #[serde(
        rename = "registeredAt",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub registered_at: Option<String>,
}

impl WorkerProfile {
    /// Load and parse the profile at `path`. A missing file yields `None`; a
    /// malformed file is treated as absent (never panics) so a corrupt profile
    /// simply re-triggers onboarding rather than wedging the worker.
    pub fn load(path: &Path) -> Option<WorkerProfile> {
        let contents = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&contents).ok()
    }

    /// Persist the profile to `path` as pretty JSON, atomically (temp file +
    /// rename) with `0600` permissions on Unix. The parent directory is created
    /// if missing.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut json = serde_json::to_string_pretty(self)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        json.push('\n');
        let pid = std::process::id();
        let tmp = path.with_extension(format!("json.tmp.{pid}"));
        std::fs::write(&tmp, json.as_bytes())?;
        set_owner_only(&tmp)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(unix)]
fn set_owner_only(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// The worker-profile file path: `<medulla-home>/worker.json` (the canonical
/// home resolver handles `MEDULLA_HOME` / `MEDULLA_DEV`).
pub fn profile_path(env: &HashMap<String, String>) -> PathBuf {
    crate::home::medulla_home(env).join("worker.json")
}

/// Whether this worker is registered: a persisted profile exists *and* a
/// tiny.place identity is present.
pub fn is_registered(profile: Option<&WorkerProfile>, identity_present: bool) -> bool {
    profile.is_some() && identity_present
}

/// The operator (username) from the environment: `USER`, else `USERNAME`, else
/// `"worker"`.
pub fn env_username(env: &HashMap<String, String>) -> String {
    for key in ["USER", "USERNAME"] {
        if let Some(value) = env.get(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    "worker".to_string()
}

/// The hostname from `HOSTNAME` in `env`, when non-empty.
pub fn env_hostname(env: &HashMap<String, String>) -> Option<String> {
    env.get("HOSTNAME")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// The hostname from the `hostname` command (first line, trimmed). Best-effort:
/// `None` when the command is unavailable or empty.
fn command_hostname() -> Option<String> {
    let output = std::process::Command::new("hostname").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let first = text.lines().next().unwrap_or("").trim();
    (!first.is_empty()).then(|| first.to_string())
}

/// Best-effort primary IPv4 via the UDP-connect trick: connecting a UDP socket to
/// a public address makes the OS pick the outbound interface without sending any
/// packets; its local address is that interface's IP. Falls back to `127.0.0.1`.
pub fn primary_ipv4() -> String {
    fn probe() -> Option<String> {
        let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
        socket.connect("8.8.8.8:80").ok()?;
        let addr = socket.local_addr().ok()?;
        Some(addr.ip().to_string())
    }
    probe().unwrap_or_else(|| "127.0.0.1".to_string())
}

/// Compose a worker name from its parts: `<user>@<host>/<ip>`. Pure; the sources
/// are resolved by [`default_worker_name`].
pub fn compose_worker_name(user: &str, host: &str, ip: &str) -> String {
    format!("{user}@{host}/{ip}")
}

/// The default worker name: `<username>@<hostname>/<ip>`. Username and hostname
/// come from `env` (falling back to the `hostname` command, then `"localhost"`);
/// the IP is a best-effort primary IPv4.
pub fn default_worker_name(env: &HashMap<String, String>) -> String {
    let user = env_username(env);
    let host = env_hostname(env)
        .or_else(command_hostname)
        .unwrap_or_else(|| "localhost".to_string());
    let ip = primary_ipv4();
    compose_worker_name(&user, &host, &ip)
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
    fn username_prefers_user_then_username_then_fallback() {
        assert_eq!(env_username(&env(&[("USER", "ada")])), "ada");
        assert_eq!(env_username(&env(&[("USERNAME", "grace")])), "grace");
        assert_eq!(
            env_username(&env(&[("USER", "ada"), ("USERNAME", "grace")])),
            "ada"
        );
        // Blank USER falls through to USERNAME.
        assert_eq!(
            env_username(&env(&[("USER", "  "), ("USERNAME", "grace")])),
            "grace"
        );
        assert_eq!(env_username(&env(&[])), "worker");
    }

    #[test]
    fn hostname_reads_env_and_skips_blank() {
        assert_eq!(
            env_hostname(&env(&[("HOSTNAME", "box-1")])).as_deref(),
            Some("box-1")
        );
        assert_eq!(env_hostname(&env(&[("HOSTNAME", "  ")])), None);
        assert_eq!(env_hostname(&env(&[])), None);
    }

    #[test]
    fn compose_matches_the_documented_shape() {
        assert_eq!(
            compose_worker_name("ada", "box", "10.0.0.4"),
            "ada@box/10.0.0.4"
        );
    }

    #[test]
    fn default_worker_name_uses_injected_env() {
        let name = default_worker_name(&env(&[("USER", "ada"), ("HOSTNAME", "box-1")]));
        // Username + hostname are deterministic; the IP part is best-effort.
        assert!(name.starts_with("ada@box-1/"), "got {name}");
        assert!(name.contains('/'), "has an ip segment: {name}");
    }

    #[test]
    fn default_worker_name_falls_back_when_env_missing() {
        // No USER/USERNAME/HOSTNAME: username defaults to "worker"; the hostname
        // may come from the `hostname` command or the "localhost" fallback.
        let name = default_worker_name(&env(&[]));
        assert!(name.starts_with("worker@"), "got {name}");
    }

    #[test]
    fn primary_ipv4_is_always_a_string() {
        // Best-effort: never panics, always returns something ip-shaped or the
        // loopback fallback.
        let ip = primary_ipv4();
        assert!(!ip.is_empty());
    }

    #[test]
    fn profile_path_is_worker_json_under_home() {
        let e = env(&[("MEDULLA_HOME", "/home/me/.medulla")]);
        let p = profile_path(&e);
        assert!(p.ends_with(".medulla/worker.json"), "got {p:?}");
    }

    #[test]
    fn profile_round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("medulla-wp-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("worker.json");
        let profile = WorkerProfile {
            name: "ada@box/10.0.0.4".to_string(),
            address: "AgentAddr111".to_string(),
            owner: Some("@overseer".to_string()),
            registered_at: Some("2026-07-18T00:00:00Z".to_string()),
        };
        profile.save(&path).unwrap();
        let loaded = WorkerProfile::load(&path).expect("profile loads");
        assert_eq!(loaded, profile);
        // The on-disk file uses the camelCase key.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"registeredAt\""), "camelCase key: {raw}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_or_corrupt_is_none() {
        assert_eq!(WorkerProfile::load(Path::new("/no/such/worker.json")), None);

        let dir = std::env::temp_dir().join(format!("medulla-wp-bad-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("worker.json");
        std::fs::write(&path, b"{ not json").unwrap();
        assert_eq!(WorkerProfile::load(&path), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn registered_requires_profile_and_identity() {
        let profile = WorkerProfile {
            name: "w".to_string(),
            ..Default::default()
        };
        assert!(is_registered(Some(&profile), true));
        assert!(!is_registered(Some(&profile), false));
        assert!(!is_registered(None, true));
        assert!(!is_registered(None, false));
    }

    #[test]
    fn owner_is_omitted_from_json_when_absent() {
        let profile = WorkerProfile {
            name: "w".to_string(),
            ..Default::default()
        };
        let json = serde_json::to_string(&profile).unwrap();
        assert!(!json.contains("owner"), "no owner key: {json}");
        assert!(
            !json.contains("registeredAt"),
            "no registeredAt key: {json}"
        );
        assert!(!json.contains("address"), "no address key: {json}");
    }
}
