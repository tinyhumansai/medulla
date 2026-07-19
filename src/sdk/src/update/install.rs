//! The thin IO half of self-update: downloading and verifying an asset,
//! extracting the archive, atomically swapping the running binary, and the
//! [`run_update`] entry point that drives `medulla update [--check]`.
//!
//! This module performs the side effects a check cannot: network downloads,
//! archive extraction via platform tools, and filesystem swaps. The pure
//! parsing/comparison core it builds on lives in [`check`](super::check).

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Result};

use super::check::{bin_name, check_for_update, sha256_hex, update_url};

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
    let nanos = crate::clock::now_nanos();
    let dir = std::env::temp_dir().join(format!("medulla-update-{}-{}", std::process::id(), nanos));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[cfg(test)]
mod install_tests {
    //! Inline tests for the module-private filesystem helpers, which the sibling
    //! `update::tests` module cannot reach.
    use super::*;

    #[test]
    fn make_workdir_creates_a_fresh_dir() {
        let dir = make_workdir().unwrap();
        assert!(dir.exists() && dir.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_binary_locates_nested_and_returns_none_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        // An empty tree has no binary.
        assert!(find_binary(dir.path()).is_none());
        // Place the platform binary a couple of levels deep, alongside a decoy.
        let nested = dir.path().join("pkg").join("bin");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(dir.path().join("README"), b"x").unwrap();
        let bin = nested.join(bin_name());
        std::fs::write(&bin, b"binary").unwrap();
        assert_eq!(find_binary(dir.path()), Some(bin));
    }

    #[test]
    fn move_file_moves_within_a_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from");
        let to = dir.path().join("to");
        std::fs::write(&from, b"payload").unwrap();
        move_file(&from, &to).unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"payload");
        assert!(!from.exists(), "source is removed after the move");
    }

    #[cfg(unix)]
    #[test]
    fn set_executable_sets_the_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("bin");
        std::fs::write(&f, b"x").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
        set_executable(&f).unwrap();
        let mode = std::fs::metadata(&f).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    #[test]
    fn install_binary_without_existing_target() {
        // When there is no target yet, no backup is created and the new binary
        // simply lands in place.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("medulla");
        let staged = dir.path().join("staged");
        std::fs::write(&staged, b"NEW").unwrap();
        install_binary(&staged, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
        assert!(!backup_path(&target).exists());
    }

    #[test]
    fn extract_archive_unpacks_a_tarball() {
        // Build a tarball with `tar` and round-trip it through extract_archive.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("hello.txt"), b"hi").unwrap();
        let archive = dir.path().join("asset.tar.gz");
        let ok = std::process::Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&src)
            .arg("hello.txt")
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            return; // `tar` unavailable — skip rather than fail.
        }
        let dest = dir.path().join("out");
        std::fs::create_dir_all(&dest).unwrap();
        extract_archive(&archive, &dest, false).unwrap();
        assert_eq!(std::fs::read(dest.join("hello.txt")).unwrap(), b"hi");
    }

    #[cfg(not(windows))]
    #[test]
    fn extract_archive_unpacks_a_zip() {
        // Build a zip with the `zip` CLI and round-trip it through the unzip path.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("payload.txt");
        std::fs::write(&src, b"zipped").unwrap();
        let archive = dir.path().join("asset.zip");
        let ok = std::process::Command::new("zip")
            .arg("-j")
            .arg(&archive)
            .arg(&src)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            return; // `zip` unavailable — skip rather than fail.
        }
        let dest = dir.path().join("out");
        std::fs::create_dir_all(&dest).unwrap();
        extract_archive(&archive, &dest, true).unwrap();
        assert_eq!(std::fs::read(dest.join("payload.txt")).unwrap(), b"zipped");
    }
}
