//! The pure core of update checking: manifest parsing, semver comparison,
//! platform selection, and the thin network fetch that resolves them into an
//! [`UpdateInfo`].
//!
//! Everything here except [`check_for_update`] is IO-free so it can be unit
//! tested without a network. [`check_for_update`] performs the single HTTP GET
//! that ties the core together.

use std::time::Duration;

use anyhow::Result;
use sha2::{Digest, Sha256};

use super::types::{Manifest, PlatformEntry, UpdateInfo, DEFAULT_UPDATE_URL};

/// Parse a manifest from JSON text.
pub fn parse_manifest(text: &str) -> Result<Manifest> {
    Ok(serde_json::from_str(text)?)
}

/// Parse a `major.minor.patch` triple, tolerating a leading `v` and any
/// pre-release/build suffix. Returns `None` for anything that is not a clean
/// three-part numeric version.
fn parse_triple(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim();
    let v = v
        .strip_prefix('v')
        .or_else(|| v.strip_prefix('V'))
        .unwrap_or(v);
    // Drop a `-pre`/`+build` suffix before splitting on dots.
    let core = v.split(['-', '+']).next().unwrap_or(v);
    let mut it = core.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None; // more than three parts — not a plain triple
    }
    Some((major, minor, patch))
}

/// Whether `latest` is a strictly newer release than `current`. Unparseable
/// input on either side is treated as "not newer" so a malformed manifest never
/// nags the user.
pub fn is_newer(current: &str, latest: &str) -> bool {
    match (parse_triple(current), parse_triple(latest)) {
        (Some(cur), Some(new)) => new > cur,
        _ => false,
    }
}

/// The compile-time target triple for the running binary, matching the keys used
/// in `latest.json` (which are the release build matrix's target triples).
pub fn platform_key() -> &'static str {
    #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
    {
        "x86_64-unknown-linux-gnu"
    }
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    {
        "aarch64-unknown-linux-gnu"
    }
    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    {
        "aarch64-apple-darwin"
    }
    #[cfg(all(target_arch = "x86_64", target_os = "windows"))]
    {
        "x86_64-pc-windows-msvc"
    }
    #[cfg(not(any(
        all(target_arch = "x86_64", target_os = "linux"),
        all(target_arch = "aarch64", target_os = "linux"),
        all(target_arch = "aarch64", target_os = "macos"),
        all(target_arch = "x86_64", target_os = "windows"),
    )))]
    {
        "unknown"
    }
}

/// The asset entry for the running platform, if the manifest ships one.
pub fn pick_platform(manifest: &Manifest) -> Option<&PlatformEntry> {
    manifest.platforms.get(platform_key())
}

/// The installed binary's file name for this platform.
pub fn bin_name() -> &'static str {
    if cfg!(windows) {
        "medulla.exe"
    } else {
        "medulla"
    }
}

/// The effective manifest URL: `MEDULLA_UPDATE_URL` (the test seam) wins,
/// otherwise the default "latest" URL.
pub fn update_url() -> String {
    std::env::var("MEDULLA_UPDATE_URL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_UPDATE_URL.to_string())
}

/// Lowercase hex SHA-256 of `data`.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Fetch the manifest and, if a newer release ships an asset for this platform,
/// return it. `Ok(None)` means "already current / nothing for this platform".
pub async fn check_for_update(url: &str, current: &str) -> Result<Option<UpdateInfo>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;
    let body = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let manifest = parse_manifest(&body)?;
    if !is_newer(current, &manifest.version) {
        return Ok(None);
    }
    let Some((url, sha256)) = pick_platform(&manifest).map(|e| (e.url.clone(), e.sha256.clone()))
    else {
        return Ok(None);
    };
    Ok(Some(UpdateInfo {
        version: manifest.version,
        tag: manifest.tag,
        notes: manifest.notes,
        url,
        sha256,
    }))
}
