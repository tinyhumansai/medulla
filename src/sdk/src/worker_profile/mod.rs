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

use crate::persistence::write_private_json;

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
        write_private_json(path, self, true)
    }
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
mod tests;

mod types;
pub use types::WorkerProfile;
