//! Read-only Git command adapter for the Changes tab.

use std::collections::BTreeMap;
#[cfg(unix)]
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use medulla::ui::git_review::ChangeOrigin;

use super::types::{ChangedFile, GitCommit, LoadedChanges};

/// Find the enclosing repository root and resolve its current commit.
pub(super) fn discover() -> Result<(PathBuf, String), String> {
    let cwd = std::env::current_dir().map_err(|error| format!("Cannot read directory: {error}"))?;
    discover_in(&cwd)
}

/// Resolve the repository root and session baseline for one directory.
///
/// A directory outside any repository is reported as a short sentence rather
/// than Git's own multi-line `fatal:` text, because it is an ordinary state for
/// the tab to sit in rather than a fault to debug. Every other discovery
/// failure (permission denied, Git not installed) is preserved verbatim, since
/// those do need debugging. A repository with no first commit is not a failure
/// at all: [`resolve_baseline`] anchors it to the empty tree so the tab reviews
/// the initial files.
pub(super) fn discover_in(directory: &Path) -> Result<(PathBuf, String), String> {
    let root = git_path(directory, &["rev-parse", "--show-toplevel"]).map_err(|error| {
        // Only the "outside a repository" case earns the friendly sentence.
        if error.to_lowercase().contains("not a git repository") {
            "Not a Git repository · nothing to review here".to_owned()
        } else {
            error
        }
    })?;
    let baseline = resolve_baseline(&root)?;
    Ok((root, baseline))
}

/// Resolve HEAD, using Git's empty tree for a repository with no first commit.
pub(super) fn resolve_baseline(root: &Path) -> Result<String, String> {
    match git(root, &["rev-parse", "--verify", "HEAD"]) {
        Ok(head) => Ok(head.trim().to_owned()),
        Err(_) => empty_tree(root),
    }
}

/// Resolve Git's canonical empty-tree object for an unborn launch snapshot.
pub(super) fn empty_tree(root: &Path) -> Result<String, String> {
    git(root, &["hash-object", "-t", "tree", "--stdin"]).map(|tree| tree.trim().to_owned())
}

/// Load commits and files changed from `baseline` through the current worktree.
pub(super) fn load(root: &Path, baseline: &str) -> Result<LoadedChanges, String> {
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
    let recent_commits = if has_head(root)? {
        parse_commits(&git(root, &["log", "-50", "--format=%H%x00%s"])?)
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

    Ok((commits, recent_commits, files))
}

/// Resolve a revision to a full commit id, rejecting trees, blobs, and typos.
pub(super) fn resolve_commit(root: &Path, revision: &str) -> Result<String, String> {
    let revision = revision.trim();
    if revision.is_empty() {
        return Err("Enter a commit id or revision".to_owned());
    }
    git(
        root,
        &["rev-parse", "--verify", &format!("{revision}^{{commit}}")],
    )
    .map(|id| id.trim().to_owned())
    .map_err(|_| format!("Unknown commit or revision: {revision}"))
}

/// Parse full-id/NUL/subject records produced by the history command.
fn parse_commits(output: &str) -> Vec<GitCommit> {
    output
        .lines()
        .filter_map(|line| {
            let (id, subject) = line.split_once('\0')?;
            Some(GitCommit {
                id: id.to_owned(),
                subject: subject.to_owned(),
            })
        })
        .collect()
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
    let mut sources: Vec<(ChangeOrigin, Vec<&str>)> = Vec::new();
    // An unborn repository has no commit to name as the range tip, and nothing
    // can have been committed during the session either, so the range is simply
    // skipped rather than asked for and failed on.
    if has_head(root)? {
        sources.push((
            ChangeOrigin::Committed,
            vec!["diff", "--name-only", "-z", &range, "--"],
        ));
    }
    sources.extend([
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
    ]);
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
        // Discovery reports this text to the operator, so a silent failure must
        // still say something rather than surfacing as an empty message.
        Err(command_error(&output))
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
