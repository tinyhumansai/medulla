//! Read-only Git command adapter for the Changes tab.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::types::ChangedFile;

/// Find the enclosing repository root and resolve its current commit.
pub(super) fn discover() -> Result<(PathBuf, String), String> {
    let cwd = std::env::current_dir().map_err(|error| format!("Cannot read directory: {error}"))?;
    let root = git(&cwd, &["rev-parse", "--show-toplevel"])?;
    let root = PathBuf::from(root.trim());
    let baseline = git(&root, &["rev-parse", "HEAD"])?;
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
    let mut files = parse_name_status(&git(root, &["diff", "--name-status", baseline])?);
    for path in git(root, &["ls-files", "--others", "--exclude-standard"])?.lines() {
        files.push(ChangedFile {
            status: "?".into(),
            path: path.into(),
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    files.dedup_by(|left, right| left.path == right.path);
    Ok((commits, files))
}

/// Return the unified patch for one path, including untracked files.
pub(super) fn patch(root: &Path, baseline: &str, path: &str) -> Result<Vec<String>, String> {
    let tracked = git(root, &["ls-files", "--error-unmatch", "--", path]).is_ok();
    let output = if tracked {
        git(
            root,
            &["diff", "--no-ext-diff", "--unified=3", baseline, "--", path],
        )?
    } else {
        let result = Command::new("git")
            .current_dir(root)
            .args([
                "diff",
                "--no-index",
                "--no-ext-diff",
                "--unified=3",
                "/dev/null",
                path,
            ])
            .output()
            .map_err(|error| format!("Cannot run git: {error}"))?;
        if !result.status.success() && result.status.code() != Some(1) {
            return Err(String::from_utf8_lossy(&result.stderr).trim().to_owned());
        }
        String::from_utf8_lossy(&result.stdout).into_owned()
    };
    Ok(output.lines().map(str::to_owned).collect())
}

/// Parse Git's tab-delimited name-status output, retaining rename destinations.
pub(super) fn parse_name_status(output: &str) -> Vec<ChangedFile> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let status = fields.next()?;
            let first = fields.next()?;
            let path = fields.next().unwrap_or(first);
            Some(ChangedFile {
                status: status.chars().next()?.to_string(),
                path: path.to_owned(),
            })
        })
        .collect()
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
