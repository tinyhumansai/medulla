//! Captures a non-reusable identity for the Git checkout behind a harness.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_MARKER: AtomicU64 = AtomicU64::new(1);

/// Create a launch marker inside Git metadata and return its opaque identity.
///
/// The marker is outside the worktree and cannot be committed. Deleting and
/// replacing the checkout removes it, even when the filesystem reuses an inode.
pub(crate) fn capture(cwd: &Path) -> Option<String> {
    let git_dir = git_dir(cwd)?;
    let nonce = format!(
        "{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos(),
        NEXT_MARKER.fetch_add(1, Ordering::Relaxed)
    );
    let directory = git_dir.join("medulla-checkouts");
    std::fs::create_dir_all(&directory).ok()?;
    std::fs::write(directory.join(&nonce), nonce.as_bytes()).ok()?;
    Some(nonce)
}

/// Verify that a launch marker still belongs to the checkout at `cwd`.
pub(crate) fn matches(cwd: &Path, identity: &str) -> bool {
    let Some(git_dir) = git_dir(cwd) else {
        return false;
    };
    std::fs::read_to_string(git_dir.join("medulla-checkouts").join(identity))
        .is_ok_and(|contents| contents == identity)
}

fn git_dir(cwd: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(["rev-parse", "--absolute-git-dir"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim().to_owned()))
}
