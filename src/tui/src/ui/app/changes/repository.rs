//! Read-only Git command adapter for the Changes tab.

#[cfg(unix)]
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::types::ChangedFile;

/// Find the enclosing repository root and resolve its current commit.
pub(super) fn discover() -> Result<(PathBuf, String), String> {
    let cwd = std::env::current_dir().map_err(|error| format!("Cannot read directory: {error}"))?;
    let root = git_path(&cwd, &["rev-parse", "--show-toplevel"])?;
    let baseline = resolve_baseline(&root)?;
    Ok((root, baseline))
}

/// Resolve HEAD, using Git's empty tree for a repository with no first commit.
pub(super) fn resolve_baseline(root: &Path) -> Result<String, String> {
    match git(root, &["rev-parse", "--verify", "HEAD"]) {
        Ok(head) => Ok(head.trim().to_owned()),
        Err(_) => {
            git(root, &["hash-object", "-t", "tree", "--stdin"]).map(|tree| tree.trim().to_owned())
        }
    }
}

/// Load commits and files changed from `baseline` through the current worktree.
pub(super) fn load(root: &Path, baseline: &str) -> Result<(Vec<String>, Vec<ChangedFile>), String> {
    let commits = if has_head(root)? {
        git(
            root,
            &["log", "--format=%h %s", &format!("{baseline}..HEAD")],
        )?
        .lines()
        .map(str::to_owned)
        .collect()
    } else {
        Vec::new()
    };
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
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    files.dedup_by(|left, right| left.path == right.path);
    Ok((commits, files))
}

/// Return the unified patch for one path, including untracked files.
pub(super) fn patch(root: &Path, baseline: &str, path: &Path) -> Result<Vec<String>, String> {
    let baseline_check = Command::new("git")
        .current_dir(root)
        .env("GIT_LITERAL_PATHSPECS", "1")
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
            .env("GIT_LITERAL_PATHSPECS", "1")
            .args(["diff", "--no-ext-diff", "--unified=3", baseline, "--"])
            .arg(path)
            .output()
            .map_err(|error| format!("Cannot run git: {error}"))?;
        command_stdout(result)?
    } else if tracked(root, path)? {
        String::new()
    } else {
        let result = Command::new("git")
            .current_dir(root)
            .args(["diff", "--no-index", "--no-ext-diff", "--unified=3", "--"])
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

/// Whether Git currently tracks `path`, independent of its diff state.
fn tracked(root: &Path, path: &Path) -> Result<bool, String> {
    let result = Command::new("git")
        .current_dir(root)
        .env("GIT_LITERAL_PATHSPECS", "1")
        .args(["ls-files", "--error-unmatch", "--"])
        .arg(path)
        .output()
        .map_err(|error| format!("Cannot run git: {error}"))?;
    match result.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(command_error(&result)),
    }
}

/// Whether the repository has a commit that can be used as the log range tip.
fn has_head(root: &Path) -> Result<bool, String> {
    let result = Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .map_err(|error| format!("Cannot run git: {error}"))?;
    match result.status.code() {
        Some(0) => Ok(true),
        Some(128) => Ok(false),
        _ => Err(command_error(&result)),
    }
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

/// Decode one path followed by Git's platform-specific line terminator.
pub(super) fn path_from_line(mut output: Vec<u8>) -> PathBuf {
    if output.last() == Some(&b'\n') {
        output.pop();
        if output.last() == Some(&b'\r') {
            output.pop();
        }
    }
    bytes_to_path(&output)
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

/// Run Git for a command whose stdout is exactly one native path.
fn git_path(root: &Path, args: &[&str]) -> Result<PathBuf, String> {
    git_bytes(root, args).map(path_from_line)
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
