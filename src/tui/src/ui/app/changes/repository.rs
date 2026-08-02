//! Read-only Git command adapter for the Changes tab.

use std::collections::BTreeMap;
#[cfg(unix)]
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use medulla::ui::git_review::ChangeOrigin;

use super::types::ChangedFile;

/// Find the enclosing repository root and resolve its current commit.
pub(super) fn discover() -> Result<(PathBuf, String), String> {
    let cwd = std::env::current_dir().map_err(|error| format!("Cannot read directory: {error}"))?;
    discover_in(&cwd)
}

/// Resolve the repository root and session baseline for one directory.
///
/// The two ways this legitimately fails — a directory outside any repository,
/// and a repository with no commit to anchor to — are reported as short
/// sentences rather than Git's own multi-line `fatal:` text, because they are
/// ordinary states for the tab to sit in rather than faults to debug.
pub(super) fn discover_in(directory: &Path) -> Result<(PathBuf, String), String> {
    let root = git(directory, &["rev-parse", "--show-toplevel"])
        .map_err(|_| "Not a Git repository · nothing to review here".to_owned())?;
    let root = PathBuf::from(root.trim());
    let baseline = git(&root, &["rev-parse", "HEAD"]).map_err(|_| {
        "This repository has no commits yet · nothing to compare against".to_owned()
    })?;
    Ok((root, baseline.trim().to_owned()))
}

/// Load commits and files changed from `baseline` through the current worktree.
pub(super) fn load(root: &Path, baseline: &str) -> Result<(Vec<String>, Vec<ChangedFile>), String> {
    let commits = git(
        root,
        &["log", "--format=%h %s", &format!("{baseline}..HEAD")],
    )?
    .lines()
    .map(str::to_owned)
    .collect();
    let mut files = parse_name_status(&git_bytes(
        root,
        &["diff", "--name-status", "-z", baseline],
    )?);
    for path in split_paths(git_bytes(
        root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?) {
        files.push(ChangedFile {
            status: "?".into(),
            path,
            origins: Vec::new(),
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    files.dedup_by(|left, right| left.path == right.path);
    let origins = origins(root, baseline)?;
    for file in &mut files {
        file.origins = origins.get(&file.path).cloned().unwrap_or_default();
    }

    // Include paths that have tracked origins (Committed, Staged, Unstaged) but are absent
    // from the aggregate diff. This can happen when a session commit changes a file and a
    // later staged or unstaged edit restores it to baseline—the cancelling changes mean
    // `git diff baseline` won't report it, but the file still has change history that should
    // be visible in the Changes tab.
    for (path, file_origins) in origins.iter() {
        if !files.iter().any(|f| &f.path == path) && !file_origins.is_empty() {
            // Only include if there are actual tracked changes (not just "Untracked")
            if file_origins.iter().any(|o| o != &ChangeOrigin::Untracked) {
                files.push(ChangedFile {
                    status: "M".into(), // Use generic "modified" status for reconstructed entries
                    path: path.clone(),
                    origins: file_origins.clone(),
                });
            }
        }
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));

    Ok((commits, files))
}

/// Classify where each changed path's content currently lives.
///
/// A path is reported once per place it differs, so a file committed during the
/// session and then edited again carries both `Committed` and `Unstaged`. This
/// is what lets the rail distinguish work the agent has already recorded from
/// edits still sitting in the index or the working tree.
pub(super) fn origins(
    root: &Path,
    baseline: &str,
) -> Result<BTreeMap<PathBuf, Vec<ChangeOrigin>>, String> {
    let mut origins: BTreeMap<PathBuf, Vec<ChangeOrigin>> = BTreeMap::new();
    let range = format!("{baseline}..HEAD");
    let sources: [(ChangeOrigin, Vec<&str>); 4] = [
        (
            ChangeOrigin::Committed,
            vec!["diff", "--name-only", "-z", &range, "--"],
        ),
        (
            ChangeOrigin::Staged,
            vec!["diff", "--name-only", "-z", "--cached", "--"],
        ),
        (
            ChangeOrigin::Unstaged,
            vec!["diff", "--name-only", "-z", "--"],
        ),
        (
            ChangeOrigin::Untracked,
            vec!["ls-files", "--others", "--exclude-standard", "-z"],
        ),
    ];
    for (origin, args) in sources {
        for path in split_paths(git_bytes(root, &args)?) {
            let entry = origins.entry(path).or_default();
            if !entry.contains(&origin) {
                entry.push(origin);
            }
        }
    }
    Ok(origins)
}

/// Return the unified patch for one path, including untracked files.
pub(super) fn patch(root: &Path, baseline: &str, path: &Path) -> Result<Vec<String>, String> {
    let baseline_check = Command::new("git")
        .current_dir(root)
        .args(["diff", "--quiet", baseline, "--"])
        .arg(path)
        .output()
        .map_err(|error| format!("Cannot run git: {error}"))?;
    let changed_at_baseline = match baseline_check.status.code() {
        Some(0) => false,
        Some(1) => true,
        _ => return Err(command_error(&baseline_check)),
    };
    let output = if changed_at_baseline {
        let result = Command::new("git")
            .current_dir(root)
            .args(["diff", "--no-ext-diff", "--unified=3", baseline, "--"])
            .arg(path)
            .output()
            .map_err(|error| format!("Cannot run git: {error}"))?;
        command_stdout(result)?
    } else {
        let result = Command::new("git")
            .current_dir(root)
            .args(["diff", "--no-index", "--no-ext-diff", "--unified=3"])
            .arg("/dev/null")
            .arg(path)
            .output()
            .map_err(|error| format!("Cannot run git: {error}"))?;
        if !result.status.success() && result.status.code() != Some(1) {
            return Err(String::from_utf8_lossy(&result.stderr).trim().to_owned());
        }
        String::from_utf8_lossy(&result.stdout).into_owned()
    };
    Ok(output.lines().map(str::to_owned).collect())
}

/// Parse Git's NUL-delimited name-status output, retaining rename destinations.
pub(super) fn parse_name_status(output: &[u8]) -> Vec<ChangedFile> {
    let mut fields = output
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    let mut files = Vec::new();
    while let Some(status) = fields.next() {
        let Some(first) = fields.next() else { break };
        let status_code = status.first().copied().map(char::from);
        let path = if matches!(status_code, Some('R' | 'C')) {
            fields.next().unwrap_or(first)
        } else {
            first
        };
        if let Some(status) = status_code {
            files.push(ChangedFile {
                status: status.to_string(),
                path: bytes_to_path(path),
                origins: Vec::new(),
            });
        }
    }
    files
}

/// Split NUL-delimited path output without applying Git's display quoting.
fn split_paths(output: Vec<u8>) -> Vec<PathBuf> {
    output
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(bytes_to_path)
        .collect()
}

#[cfg(unix)]
fn bytes_to_path(path: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    PathBuf::from(OsString::from_vec(path.to_vec()))
}

#[cfg(not(unix))]
fn bytes_to_path(path: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(path).into_owned())
}

/// Run a read-only Git command and return UTF-8-lossy stdout.
fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .map_err(|error| format!("Cannot run git: {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if detail.is_empty() {
            "Git command failed".into()
        } else {
            detail
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Run Git and preserve stdout bytes for path-oriented commands.
fn git_bytes(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .map_err(|error| format!("Cannot run git: {error}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

/// Accept a successful command and decode its non-path payload.
fn command_stdout(output: std::process::Output) -> Result<String, String> {
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

/// Decode a Git subprocess failure, supplying a useful fallback when silent.
fn command_error(output: &std::process::Output) -> String {
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if detail.is_empty() {
        "Git command failed".into()
    } else {
        detail
    }
}
