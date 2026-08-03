//! Reading, writing and locking `<home>/link/node.json`.
//!
//! The write is atomic (temp sibling then rename) and `0600`, and the lock is
//! the advisory whole-file lock the tiny.place identity bootstrap uses: held for
//! the life of the guard, released by the OS if the process dies.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use fs2::FileExt;

use super::types::{KeyError, KeyResult, NodeLock, NodeState};

/// The identity file inside the link directory.
pub const NODE_FILE: &str = "node.json";

/// The lock file guarding it.
pub const LOCK_FILE: &str = "node.lock";

/// `<dir>/node.json`.
pub fn node_path(dir: &Path) -> PathBuf {
    dir.join(NODE_FILE)
}

/// Read and parse the node state at `path`.
///
/// # Errors
///
/// [`KeyError::NotEnrolled`] when the file does not exist, [`KeyError::Malformed`]
/// when it does not parse. A malformed file is never silently replaced: it holds
/// the only copy of a pair key that cannot be recovered from anywhere else.
pub fn read_node_state(path: &Path) -> KeyResult<NodeState> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(KeyError::NotEnrolled(path.to_path_buf()))
        }
        Err(error) => return Err(KeyError::Io(error)),
    };
    serde_json::from_str(&contents).map_err(|error| KeyError::Malformed {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
}

/// Persist `state` to `path` atomically, owner-readable only.
///
/// The temp sibling is created with the restricted mode *before* any bytes are
/// written, so the pair key is never briefly world-readable on disk.
pub fn write_node_state(path: &Path, state: &NodeState) -> KeyResult<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut bytes = serde_json::to_vec_pretty(state)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    bytes.push(b'\n');

    let temporary = path.with_file_name(format!(
        ".{}.tmp.{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(NODE_FILE),
        std::process::id()
    ));
    let result = (|| -> std::io::Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        set_owner_only(&temporary)?;
        replace(&temporary, path)?;
        sync_parent(path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    Ok(result?)
}

/// Make the renamed directory entry durable as well as the file contents.
#[cfg(unix)]
fn sync_parent(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Take the exclusive lock on `<dir>/node.lock`, creating the directory if
/// needed.
///
/// # Errors
///
/// [`KeyError::Busy`] when another process holds it — never a fallback to a
/// different identity, because a link identity is not interchangeable: the peer
/// is talking to *this* node id.
pub fn lock_node_dir(dir: &Path) -> KeyResult<NodeLock> {
    std::fs::create_dir_all(dir)?;
    let lock_path = dir.join(LOCK_FILE);
    let file: File = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(NodeLock::new(file, lock_path)),
        Err(error) if is_contended(&error) => Err(KeyError::Busy(lock_path)),
        Err(error) => Err(KeyError::Io(error)),
    }
}

/// Whether a failed lock attempt means "someone else holds it" rather than a
/// real I/O failure.
///
/// The raw errno is compared rather than [`std::io::ErrorKind`] because the
/// Windows contention code has no dedicated kind and would otherwise be
/// indistinguishable from any other uncategorised failure.
fn is_contended(error: &std::io::Error) -> bool {
    let sentinel = fs2::lock_contended_error();
    match (error.raw_os_error(), sentinel.raw_os_error()) {
        (Some(actual), Some(expected)) => actual == expected,
        _ => error.kind() == sentinel.kind(),
    }
}

/// Replace the target with the prepared sibling.
#[cfg(not(windows))]
fn replace(temporary: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::rename(temporary, target)
}

/// Windows cannot rename over an existing file through `std`; remove it first.
#[cfg(windows)]
fn replace(temporary: &Path, target: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(target) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    std::fs::rename(temporary, target)
}

/// Restrict the persisted identity to its owner.
#[cfg(unix)]
fn set_owner_only(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

/// Platforms without Unix mode bits need no permission adjustment.
#[cfg(not(unix))]
fn set_owner_only(_path: &Path) -> std::io::Result<()> {
    Ok(())
}
