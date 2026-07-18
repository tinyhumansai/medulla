//! Release update checking and self-update.
//!
//! The published release workflow attaches a `latest.json` manifest to the
//! GitHub "latest" release, so `releases/latest/download/latest.json` always
//! serves the newest version's shape:
//!
//! ```json
//! {
//!   "version": "3.8.0",
//!   "tag": "v3.8.0",
//!   "pubDate": "2026-07-18T00:00:00Z",
//!   "notes": "https://github.com/tinyhumansai/medulla/releases/tag/v3.8.0",
//!   "platforms": {
//!     "aarch64-apple-darwin": { "url": "https://.../medulla-v3.8.0-aarch64-apple-darwin.tar.gz", "sha256": "<hex>" }
//!   }
//! }
//! ```
//!
//! The pure core (parsing, semver comparison, platform selection) is separated
//! from the thin IO (HTTP GET, download + extract + atomic replace) so it can be
//! unit-tested without a network or a real binary swap.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The default manifest URL: the "latest" release always redirects here.
pub const DEFAULT_UPDATE_URL: &str =
    "https://github.com/tinyhumansai/medulla/releases/latest/download/latest.json";

/// One platform's downloadable asset in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformEntry {
    pub url: String,
    pub sha256: String,
}

/// The `latest.json` release manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub pub_date: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub platforms: HashMap<String, PlatformEntry>,
}

/// A resolved, actionable update for the running platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    pub version: String,
    pub tag: String,
    pub notes: String,
    pub url: String,
    pub sha256: String,
}

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

/// Download `url` into `workdir`, verify its SHA-256, extract the archive, and
/// return the path to the extracted `medulla` binary. Used by the self-updater
/// and exercised directly by the e2e suite against a stub server + fixture.
pub async fn download_and_stage(
    url: &str,
    sha256_expected: &str,
    workdir: &Path,
) -> Result<PathBuf> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()?;
    let bytes = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let got = sha256_hex(&bytes);
    if !got.eq_ignore_ascii_case(sha256_expected) {
        bail!("sha256 mismatch: expected {sha256_expected}, got {got}");
    }
    let is_zip = url.ends_with(".zip");
    let archive = workdir.join(if is_zip { "asset.zip" } else { "asset.tar.gz" });
    std::fs::write(&archive, &bytes)?;
    let extract_dir = workdir.join("extract");
    std::fs::create_dir_all(&extract_dir)?;
    extract_archive(&archive, &extract_dir, is_zip)?;
    find_binary(&extract_dir).ok_or_else(|| anyhow!("no `{}` binary found in archive", bin_name()))
}

/// Extract `archive` into `dest` using dep-free platform tools: `tar` for
/// tarballs everywhere, PowerShell `Expand-Archive` (Windows) or `unzip`
/// (unix) for zips.
fn extract_archive(archive: &Path, dest: &Path, is_zip: bool) -> Result<()> {
    use std::process::Command;
    let status = if is_zip {
        #[cfg(windows)]
        {
            Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    &format!(
                        "Expand-Archive -Force -Path '{}' -DestinationPath '{}'",
                        archive.display(),
                        dest.display()
                    ),
                ])
                .status()?
        }
        #[cfg(not(windows))]
        {
            Command::new("unzip")
                .arg("-o")
                .arg(archive)
                .arg("-d")
                .arg(dest)
                .status()?
        }
    } else {
        Command::new("tar")
            .arg("-xzf")
            .arg(archive)
            .arg("-C")
            .arg(dest)
            .status()?
    };
    if !status.success() {
        bail!("archive extraction failed (exit {status})");
    }
    Ok(())
}

/// Recursively locate the platform binary under `dir`.
fn find_binary(dir: &Path) -> Option<PathBuf> {
    let target = bin_name();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().and_then(|n| n.to_str()) == Some(target) {
                return Some(p);
            }
        }
    }
    None
}

/// Whether we can create a file alongside `exe` (i.e. install over it).
pub fn exe_is_writable(exe: &Path) -> bool {
    let parent = exe.parent().unwrap_or_else(|| Path::new("."));
    let probe = parent.join(format!(".medulla-update-probe-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Atomically replace `target_exe` with `new_bin`: move the current binary aside
/// to `<exe>.old`, move the new one into place, and restore the backup on
/// failure. The `<exe>.old` file is intentionally left behind for rollback.
pub fn install_binary(new_bin: &Path, target_exe: &Path) -> Result<()> {
    #[cfg(unix)]
    set_executable(new_bin)?;

    let backup = backup_path(target_exe);
    let had_target = target_exe.exists();
    if had_target {
        let _ = std::fs::remove_file(&backup);
        std::fs::rename(target_exe, &backup).map_err(|e| {
            anyhow!(
                "cannot move current binary aside ({e}) — is {} writable?",
                target_exe.display()
            )
        })?;
    }
    match move_file(new_bin, target_exe) {
        Ok(()) => Ok(()),
        Err(e) => {
            if had_target {
                let _ = std::fs::rename(&backup, target_exe);
            }
            Err(anyhow!("failed to install new binary ({e})"))
        }
    }
}

/// The rollback path for an installed binary (`<exe>.old`).
pub fn backup_path(target_exe: &Path) -> PathBuf {
    let mut name = target_exe
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".old");
    target_exe.with_file_name(name)
}

/// Move a file, falling back to copy+remove across filesystem boundaries.
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(from, to)?;
            let _ = std::fs::remove_file(from);
            Ok(())
        }
    }
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

/// Drive `medulla update [--check]`: report the result and, unless `check_only`,
/// download + verify + install the new binary over the running executable.
pub async fn run_update(check_only: bool) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let url = update_url();
    match check_for_update(&url, current).await? {
        None => {
            println!("medulla {current} is up to date.");
            Ok(())
        }
        Some(info) if check_only => {
            println!(
                "update available: medulla {} (current {current})",
                info.version
            );
            if !info.notes.is_empty() {
                println!("release notes: {}", info.notes);
            }
            println!("run `medulla update` to install.");
            Ok(())
        }
        Some(info) => {
            let exe = std::env::current_exe()?;
            if !exe_is_writable(&exe) {
                bail!(
                    "{} is not writable — reinstall through your package manager or rerun with write access",
                    exe.display()
                );
            }
            println!("downloading medulla {}…", info.version);
            let workdir = make_workdir()?;
            let staged = match download_and_stage(&info.url, &info.sha256, &workdir).await {
                Ok(p) => p,
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&workdir);
                    return Err(e);
                }
            };
            let result = install_binary(&staged, &exe);
            let _ = std::fs::remove_dir_all(&workdir);
            result?;
            println!(
                "updated to medulla {}. Restart medulla to use it.",
                info.version
            );
            println!(
                "(previous binary kept at {} — delete it once the update looks good)",
                backup_path(&exe).display()
            );
            Ok(())
        }
    }
}

fn make_workdir() -> Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("medulla-update-{}-{}", std::process::id(), nanos));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_compare_matrix() {
        // newer
        assert!(is_newer("1.2.3", "1.2.4"));
        assert!(is_newer("1.2.3", "1.3.0"));
        assert!(is_newer("1.2.3", "2.0.0"));
        // leading-v tolerance on both sides
        assert!(is_newer("v1.2.3", "v1.2.4"));
        assert!(is_newer("1.2.3", "v2.0.0"));
        // equal / older → not newer
        assert!(!is_newer("1.2.3", "1.2.3"));
        assert!(!is_newer("2.0.0", "1.9.9"));
        assert!(!is_newer("1.2.4", "1.2.3"));
        // pre-release suffix on the core triple is ignored
        assert!(!is_newer("1.2.3", "1.2.3-rc1"));
        // garbage on either side → never newer
        assert!(!is_newer("1.2.3", "not-a-version"));
        assert!(!is_newer("garbage", "1.2.3"));
        assert!(!is_newer("1.2", "1.2.0"));
        assert!(!is_newer("1.2.3", "1.2.3.4"));
    }

    #[test]
    fn manifest_parse_and_platform_pick() {
        let json = r#"{
            "version": "3.9.0",
            "tag": "v3.9.0",
            "pubDate": "2026-07-18T00:00:00Z",
            "notes": "https://example/notes",
            "platforms": {
                "aarch64-apple-darwin": {"url": "https://example/a.tar.gz", "sha256": "aa"},
                "x86_64-pc-windows-msvc": {"url": "https://example/w.zip", "sha256": "bb"}
            }
        }"#;
        let m = parse_manifest(json).unwrap();
        assert_eq!(m.version, "3.9.0");
        assert_eq!(m.tag, "v3.9.0");
        assert_eq!(m.pub_date, "2026-07-18T00:00:00Z");
        assert_eq!(m.platforms.len(), 2);
        let entry = m.platforms.get("aarch64-apple-darwin").unwrap();
        assert_eq!(entry.url, "https://example/a.tar.gz");
        assert_eq!(entry.sha256, "aa");
    }

    #[test]
    fn platform_key_is_a_known_triple_or_unknown() {
        let key = platform_key();
        let known = [
            "x86_64-unknown-linux-gnu",
            "aarch64-unknown-linux-gnu",
            "aarch64-apple-darwin",
            "x86_64-pc-windows-msvc",
            "unknown",
        ];
        assert!(known.contains(&key), "unexpected platform key: {key}");
    }

    #[test]
    fn pick_platform_absent_is_none() {
        let m = Manifest {
            version: "9.9.9".into(),
            tag: "v9.9.9".into(),
            pub_date: String::new(),
            notes: String::new(),
            platforms: HashMap::new(),
        };
        assert!(pick_platform(&m).is_none());
    }

    #[test]
    fn sha256_matches_known_vector() {
        // SHA-256 of the empty string.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        // SHA-256 of "abc".
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn backup_path_appends_old() {
        let bp = backup_path(Path::new("/usr/local/bin/medulla"));
        assert_eq!(bp, PathBuf::from("/usr/local/bin/medulla.old"));
    }

    #[test]
    fn install_binary_replaces_and_backs_up() {
        let dir = std::env::temp_dir().join(format!(
            "medulla-install-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("medulla");
        std::fs::write(&target, b"OLD").unwrap();
        let new = dir.join("staged");
        std::fs::write(&new, b"NEW").unwrap();

        install_binary(&new, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
        assert_eq!(std::fs::read(backup_path(&target)).unwrap(), b"OLD");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
