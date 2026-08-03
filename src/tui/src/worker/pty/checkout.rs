//! Captures filesystem identity for the Git directory behind a harness checkout.

use std::path::Path;
use std::process::Command;

/// Return an identity that changes when a checkout is deleted and replaced.
pub(crate) fn identity(cwd: &Path) -> Option<String> {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(["rev-parse", "--absolute-git-dir"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = std::str::from_utf8(&output.stdout).ok()?;
    let metadata = Path::new(text.trim()).metadata().ok()?;
    metadata_identity(&metadata)
}

#[cfg(unix)]
fn metadata_identity(metadata: &std::fs::Metadata) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    Some(format!("{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn metadata_identity(metadata: &std::fs::Metadata) -> Option<String> {
    use std::os::windows::fs::MetadataExt;
    Some(format!(
        "{}:{}",
        metadata.volume_serial_number()?,
        metadata.file_index()?
    ))
}

#[cfg(not(any(unix, windows)))]
fn metadata_identity(metadata: &std::fs::Metadata) -> Option<String> {
    Some(format!("{:?}", metadata.modified().ok()?))
}
