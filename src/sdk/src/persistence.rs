//! Shared atomic file persistence.
//!
//! Configuration, worker identity, and Signal session state all need the same
//! write-sibling-then-rename discipline. This module owns collision-resistant
//! temporary paths, cleanup, and private-file permissions so callers cannot
//! accidentally weaken one store while fixing another.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

/// Monotonic suffix preventing same-process writers from sharing a temp file.
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Atomically replace `path` with `bytes`.
///
/// Data is written and synced through a unique sibling before rename. A failed
/// write removes its temporary file and leaves an existing target untouched.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_with(path, bytes, |_| Ok(()))
}

/// Serialize `value` as JSON and atomically persist it with owner-only
/// permissions on Unix.
pub(crate) fn write_private_json<T: Serialize>(
    path: &Path,
    value: &T,
    pretty: bool,
) -> io::Result<()> {
    let mut bytes = if pretty {
        serde_json::to_vec_pretty(value)
    } else {
        serde_json::to_vec(value)
    }
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if pretty {
        bytes.push(b'\n');
    }
    write_atomic_with(path, &bytes, set_owner_only)
}

/// Write through a sibling and apply `prepare` before the atomic replacement.
fn write_atomic_with(
    path: &Path,
    bytes: &[u8],
    prepare: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = temporary_path(path);
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        prepare(&temporary)?;
        replace(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// A unique sibling path that never depends on the target's extension.
fn temporary_path(path: &Path) -> PathBuf {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("medulla-state");
    path.with_file_name(format!(".{name}.tmp.{}.{}", std::process::id(), sequence))
}

/// Replace the target with the prepared sibling.
#[cfg(not(windows))]
fn replace(temporary: &Path, target: &Path) -> io::Result<()> {
    std::fs::rename(temporary, target)
}

/// Windows cannot rename over an existing file through `std`; remove it first.
#[cfg(windows)]
fn replace(temporary: &Path, target: &Path) -> io::Result<()> {
    match std::fs::remove_file(target) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    std::fs::rename(temporary, target)
}

/// Restrict a persisted identity file to its owner.
#[cfg(unix)]
fn set_owner_only(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

/// Platforms without Unix mode bits need no permission adjustment.
#[cfg(not(unix))]
fn set_owner_only(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_and_leaves_no_sibling() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, "old").unwrap();
        write_atomic(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn private_json_is_pretty_and_newline_terminated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.json");
        write_private_json(&path, &serde_json::json!({"name": "worker"}), true).unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains('\n'));
        assert!(text.ends_with('\n'));
    }

    #[cfg(unix)]
    #[test]
    fn private_json_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.json");
        write_private_json(&path, &serde_json::json!({}), false).unwrap();
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
