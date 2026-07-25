//! CLI config-file model and endpoint resolution.
//!
//! The config file (`TINYPLACE_CONFIG`, else `<medulla_home>/tinyplace/config.json`) holds
//! the endpoint, identity key, SIWS proof, and OpenHuman owner. This module only
//! reads and models it; it does not generate keys or write files. Environment
//! lookups are passed in (not read from `std::env`) so resolution is testable.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Fallback API endpoint when none is configured. Aliases the single
/// source of truth in [`crate::config`] so the prod tiny.place URL is not
/// duplicated here.
pub const DEFAULT_ENDPOINT: &str = crate::config::PROD_TINYPLACE_BASE_URL;

/// The absolute config-file path: `TINYPLACE_CONFIG` when set, otherwise
/// `<medulla_home>/tinyplace/config.json`.
///
/// The identity lives under the Medulla home, not `~/.tinyplace`, so one
/// directory holds everything Medulla owns — wallet, Signal store, credentials,
/// and config — and Medulla drives every handshake from keys it controls.
/// `~/.tinyplace` belongs to the standalone tiny.place CLI; sharing it made the
/// two tools contend over one wallet and one Signal ratchet.
///
/// `home_dir` is the fallback base used only when the injected environment
/// carries no `HOME`/`USERPROFILE`, keeping resolution pure and testable.
pub fn config_path(env: &HashMap<String, String>, home_dir: &Path) -> PathBuf {
    if let Some(path) = env.get("TINYPLACE_CONFIG").filter(|p| !p.is_empty()) {
        return PathBuf::from(path);
    }
    let home = if env.contains_key("MEDULLA_HOME")
        || env.contains_key("HOME")
        || env.contains_key("USERPROFILE")
        || env.get("MEDULLA_DEV").map(|v| crate::home::is_truthy(v)) == Some(true)
    {
        crate::home::medulla_home(env)
    } else {
        home_dir.join(".medulla")
    };
    home.join("tinyplace").join("config.json")
}

/// Parse config JSON into a [`TinyplaceFileConfig`]. A non-object or a malformed
/// document yields an empty config rather than an error, keeping only the keys
/// this crate recognizes.
pub fn parse_config(contents: &str) -> TinyplaceFileConfig {
    let value: serde_json::Value = match serde_json::from_str(contents) {
        Ok(value) => value,
        Err(_) => return TinyplaceFileConfig::default(),
    };
    if !value.is_object() {
        return TinyplaceFileConfig::default();
    }
    serde_json::from_value(value).unwrap_or_default()
}

/// Load and parse the config at `path`. A missing file (or any read error) is
/// treated as an empty config; this never panics.
pub fn load_config(path: &Path) -> TinyplaceFileConfig {
    match std::fs::read_to_string(path) {
        Ok(contents) => parse_config(&contents),
        Err(_) => TinyplaceFileConfig::default(),
    }
}

/// Persist `config` to `path` as pretty JSON, atomically (write a sibling temp
/// file, then rename over the target) and with `0600` permissions on Unix. The
/// parent directory is created if missing. Mirrors the tinyplace CLI config
/// model so the file stays interoperable with the SDK/CLI.
pub fn write_config(path: &Path, config: &TinyplaceFileConfig) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut json = serde_json::to_string_pretty(config)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    json.push('\n');

    let pid = std::process::id();
    let tmp = path.with_extension(format!("json.tmp.{pid}"));
    std::fs::write(&tmp, json.as_bytes())?;
    set_owner_only(&tmp)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
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

/// Resolve the API endpoint. Order: `TINYPLACE_ENDPOINT` > `TINYPLACE_API_URL` >
/// `NEXT_PUBLIC_API_URL` > `config.endpoint` > the staging/prod default. The
/// default is [`DEFAULT_ENDPOINT`] (prod), or the staging tiny.place URL when
/// `MEDULLA_STAGING` is truthy.
pub fn resolve_endpoint(env: &HashMap<String, String>, config: &TinyplaceFileConfig) -> String {
    for key in [
        "TINYPLACE_ENDPOINT",
        "TINYPLACE_API_URL",
        "NEXT_PUBLIC_API_URL",
    ] {
        if let Some(value) = env.get(key).map(|v| v.trim()).filter(|v| !v.is_empty()) {
            return value.to_string();
        }
    }
    // Delegate the `config.endpoint` → staging/prod-default tail to the shared
    // resolver so the "explicit value else default" logic (and its trim policy)
    // lives in exactly one place.
    crate::config::resolve_tinyplace_base_url(env, config.endpoint.as_deref())
}

#[cfg(test)]
mod tests;

mod types;
pub use types::TinyplaceFileConfig;
