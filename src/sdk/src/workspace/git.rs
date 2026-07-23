//! Shell-free wrappers around read-only Git commands.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use super::types::{BranchState, CommitSummary, FileChange, WorkspaceError, WorkspaceSnapshot};

/// Run Git in `workspace`, preserving every argument as a separate process arg.
fn run_git<I, S>(
    workspace: &Path,
    operation: &'static str,
    args: I,
) -> Result<Output, WorkspaceError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(args)
        .output()
        .map_err(|source| WorkspaceError::Spawn { operation, source })?;
    if output.status.success() {
        return Ok(output);
    }
    Err(WorkspaceError::Git {
        operation,
        workspace: workspace.to_path_buf(),
        message: String::from_utf8_lossy(&output.stderr)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
    })
}

/// Run Git but preserve a non-zero status for commands with meaningful misses.
fn try_git<I, S>(
    workspace: &Path,
    operation: &'static str,
    args: I,
) -> Result<Output, WorkspaceError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(args)
        .output()
        .map_err(|source| WorkspaceError::Spawn { operation, source })
}

/// Read the current branch (or detached commit) and upstream divergence.
pub fn current_branch(workspace: &Path) -> Result<BranchState, WorkspaceError> {
    // Validate the root first so a non-repository is not misreported as a
    // detached repository merely because `symbolic-ref` returned non-zero.
    run_git(
        workspace,
        "repository discovery",
        ["rev-parse", "--show-toplevel"],
    )?;
    let symbolic = try_git(
        workspace,
        "branch inspection",
        ["symbolic-ref", "--quiet", "--short", "HEAD"],
    )?;
    let (name, detached) = if symbolic.status.success() {
        (
            String::from_utf8_lossy(&symbolic.stdout).trim().to_owned(),
            false,
        )
    } else {
        let output = run_git(
            workspace,
            "detached HEAD inspection",
            ["rev-parse", "--short", "HEAD"],
        )?;
        (
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
            true,
        )
    };

    let divergence = try_git(
        workspace,
        "upstream divergence",
        ["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
    )?;
    let (ahead, behind) = if divergence.status.success() {
        let text = String::from_utf8_lossy(&divergence.stdout);
        let mut parts = text.split_whitespace();
        let ahead = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let behind = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        (ahead, behind)
    } else {
        (0, 0)
    };

    Ok(BranchState {
        name,
        detached,
        ahead,
        behind,
    })
}

/// Parse `git status --porcelain=v1 -z`, retaining rename origins.
pub fn status_porcelain(workspace: &Path) -> Result<Vec<FileChange>, WorkspaceError> {
    let output = run_git(
        workspace,
        "status",
        ["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    parse_status(workspace, &output.stdout)
}

/// Parse Git's NUL-delimited porcelain output independently of process I/O.
pub(super) fn parse_status(
    workspace: &Path,
    output: &[u8],
) -> Result<Vec<FileChange>, WorkspaceError> {
    let mut fields = output.split(|byte| *byte == 0).peekable();
    let mut changes = Vec::new();
    while let Some(field) = fields.next() {
        if field.is_empty() {
            continue;
        }
        if field.len() < 4 || field[2] != b' ' {
            return Err(WorkspaceError::Malformed {
                operation: "status",
                workspace: workspace.to_path_buf(),
                message: "porcelain record is shorter than the XY prefix".into(),
            });
        }
        let index_status = field[0] as char;
        let worktree_status = field[1] as char;
        let path = PathBuf::from(String::from_utf8_lossy(&field[3..]).into_owned());
        let renamed = matches!(index_status, 'R' | 'C') || matches!(worktree_status, 'R' | 'C');
        let original_path = if renamed {
            let Some(original) = fields.next().filter(|value| !value.is_empty()) else {
                return Err(WorkspaceError::Malformed {
                    operation: "status",
                    workspace: workspace.to_path_buf(),
                    message: "rename record has no original path".into(),
                });
            };
            Some(PathBuf::from(
                String::from_utf8_lossy(original).into_owned(),
            ))
        } else {
            None
        };
        changes.push(FileChange {
            path,
            original_path,
            index_status,
            worktree_status,
        });
    }
    changes.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(changes)
}

/// Return paths changed in either the index or worktree, without duplicates.
pub fn diff_name_only(workspace: &Path) -> Result<Vec<PathBuf>, WorkspaceError> {
    let mut paths = BTreeSet::new();
    for args in [
        ["diff", "--name-only", "-z"].as_slice(),
        ["diff", "--cached", "--name-only", "-z"].as_slice(),
    ] {
        let output = run_git(workspace, "diff names", args)?;
        for field in output.stdout.split(|byte| *byte == 0) {
            if !field.is_empty() {
                paths.insert(PathBuf::from(String::from_utf8_lossy(field).into_owned()));
            }
        }
    }
    Ok(paths.into_iter().collect())
}

/// Return staged and unstaged patches for one repository-relative path.
pub fn diff(workspace: &Path, path: &Path) -> Result<String, WorkspaceError> {
    let mut sections = Vec::new();
    for (label, cached) in [("STAGED", true), ("WORKTREE", false)] {
        let mut command = Command::new("git");
        command.arg("-C").arg(workspace).arg("diff");
        if cached {
            command.arg("--cached");
        }
        let output =
            command
                .arg("--")
                .arg(path)
                .output()
                .map_err(|source| WorkspaceError::Spawn {
                    operation: "path diff",
                    source,
                })?;
        if !output.status.success() {
            return Err(WorkspaceError::Git {
                operation: "path diff",
                workspace: workspace.to_path_buf(),
                message: String::from_utf8_lossy(&output.stderr)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
            });
        }
        if !output.stdout.is_empty() {
            sections.push(format!(
                "--- {label} ---\n{}",
                String::from_utf8_lossy(&output.stdout)
            ));
        }
    }
    Ok(sections.join("\n"))
}

/// Return up to `limit` recent commits, newest first.
pub fn log_recent(workspace: &Path, limit: usize) -> Result<Vec<CommitSummary>, WorkspaceError> {
    let format = "%H%x1f%h%x1f%an%x1f%at%x1f%s%x00";
    let output = run_git(
        workspace,
        "log",
        [
            "log".to_owned(),
            format!("--max-count={limit}"),
            format!("--format={format}"),
        ],
    )?;
    parse_log(workspace, &output.stdout)
}

/// Parse Git's record/field-delimited log format independently of process I/O.
pub(super) fn parse_log(
    workspace: &Path,
    output: &[u8],
) -> Result<Vec<CommitSummary>, WorkspaceError> {
    let mut commits = Vec::new();
    for record in output.split(|byte| *byte == 0) {
        let record = String::from_utf8_lossy(record);
        let record = record.trim_start_matches('\n').trim();
        if record.is_empty() {
            continue;
        }
        let fields: Vec<&str> = record.split('\x1f').collect();
        if fields.len() != 5 {
            return Err(WorkspaceError::Malformed {
                operation: "log",
                workspace: workspace.to_path_buf(),
                message: format!("expected 5 fields, got {}", fields.len()),
            });
        }
        let timestamp = fields[3].parse().map_err(|_| WorkspaceError::Malformed {
            operation: "log",
            workspace: workspace.to_path_buf(),
            message: "commit timestamp is not an integer".into(),
        })?;
        commits.push(CommitSummary {
            id: fields[0].to_owned(),
            short_id: fields[1].to_owned(),
            author: fields[2].to_owned(),
            timestamp,
            subject: fields[4].to_owned(),
        });
    }
    Ok(commits)
}

/// Load the branch, status, and recent history for one workspace.
pub fn inspect_workspace(workspace: &Path) -> Result<WorkspaceSnapshot, WorkspaceError> {
    Ok(WorkspaceSnapshot {
        root: workspace.to_path_buf(),
        branch: current_branch(workspace)?,
        files: status_porcelain(workspace)?,
        commits: log_recent(workspace, 8)?,
    })
}
