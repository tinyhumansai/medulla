//! Data types returned by local Git workspace inspection.

use std::path::PathBuf;

use thiserror::Error;

/// Policy applied by the exact-path committer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommitOptions {
    /// Optional body supplied as a second commit-message paragraph.
    pub body: Option<String>,
    /// Repository-relative glob patterns that require an explicit override.
    pub shared_path_denylist: Vec<String>,
    /// Permit paths matched by `shared_path_denylist`.
    pub allow_shared: bool,
}

/// The commit created by [`super::commit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitOutcome {
    /// Full commit id.
    pub id: String,
    /// Abbreviated commit id.
    pub short_id: String,
    /// Validated first line supplied by the caller.
    pub subject: String,
    /// Paths included in the commit, sorted and deduplicated.
    pub paths: Vec<PathBuf>,
}

/// A refusal or Git failure from an exact-path commit.
#[derive(Debug, Error)]
pub enum CommitError {
    /// No paths were named.
    #[error("commit refused: name at least one changed path")]
    EmptyPaths,
    /// A path was absolute or escaped the repository root.
    #[error("commit refused: path must be repository-relative and cannot escape the root: {0}")]
    UnsafePath(PathBuf),
    /// A named path has no staged, unstaged, or untracked change.
    #[error("commit refused: named path is not modified: {0}")]
    Unmodified(PathBuf),
    /// The index contains a staged path outside the requested boundary.
    #[error("commit refused: another path is already staged: {0}")]
    ForeignStaged(PathBuf),
    /// The subject is not a conventional commit subject.
    #[error("commit refused: subject must use conventional commit form, for example feat(scope): summary")]
    InvalidSubject,
    /// A guarded shared path was named without the explicit override.
    #[error("commit refused: shared path requires --allow-shared: {path} (matched {pattern})")]
    SharedPath {
        /// Guarded repository-relative path.
        path: PathBuf,
        /// Configured pattern that matched it.
        pattern: String,
    },
    /// A configured denylist pattern was invalid.
    #[error("commit refused: invalid shared-path pattern {pattern}: {message}")]
    InvalidPattern {
        /// Invalid configured glob.
        pattern: String,
        /// Parser diagnostic.
        message: String,
    },
    /// The configured Git executable could not be started.
    #[error("could not run git for {operation}: {source}")]
    Spawn {
        /// Operation being attempted.
        operation: &'static str,
        /// Process-spawn failure.
        #[source]
        source: std::io::Error,
    },
    /// Git rejected an operation.
    #[error("git {operation} failed in {workspace}: {message}")]
    Git {
        /// Operation being attempted.
        operation: &'static str,
        /// Repository root.
        workspace: PathBuf,
        /// Normalized Git stderr.
        message: String,
    },
}

/// A typed failure from a Git workspace operation.
#[derive(Debug, Error)]
pub enum WorkspaceError {
    /// The configured Git executable could not be started.
    #[error("could not run git for {operation}: {source}")]
    Spawn {
        /// The operation being attempted.
        operation: &'static str,
        /// The process-spawn failure.
        #[source]
        source: std::io::Error,
    },
    /// Git rejected an operation, commonly because the path is not a repository.
    #[error("git {operation} failed in {workspace}: {message}")]
    Git {
        /// The operation being attempted.
        operation: &'static str,
        /// The configured workspace root.
        workspace: PathBuf,
        /// Git's stderr, normalized to one line.
        message: String,
    },
    /// Git returned output that did not match its documented machine format.
    #[error("git returned malformed {operation} output in {workspace}: {message}")]
    Malformed {
        /// The operation being parsed.
        operation: &'static str,
        /// The configured workspace root.
        workspace: PathBuf,
        /// What was malformed.
        message: String,
    },
}

/// The currently checked-out branch, or detached commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchState {
    /// A branch name, or a short commit id while detached.
    pub name: String,
    /// Whether `HEAD` is detached.
    pub detached: bool,
    /// Commits the local branch is ahead of its configured upstream.
    pub ahead: usize,
    /// Commits the local branch is behind its configured upstream.
    pub behind: usize,
}

/// One changed path from Git porcelain status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// Current repository-relative path.
    pub path: PathBuf,
    /// Previous path for a rename/copy.
    pub original_path: Option<PathBuf>,
    /// Index status character (`' '` means unchanged).
    pub index_status: char,
    /// Worktree status character (`' '` means unchanged).
    pub worktree_status: char,
}

impl FileChange {
    /// Git's compact two-character status marker.
    pub fn marker(&self) -> String {
        format!("{}{}", self.index_status, self.worktree_status)
    }
}

/// One recent commit from the workspace history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitSummary {
    /// Full commit id.
    pub id: String,
    /// Abbreviated commit id.
    pub short_id: String,
    /// Commit author display name.
    pub author: String,
    /// Unix timestamp in seconds.
    pub timestamp: i64,
    /// First line of the commit message.
    pub subject: String,
}

/// The complete read-only ledger view for one local repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSnapshot {
    /// Configured workspace root.
    pub root: PathBuf,
    /// Current branch/detached state and upstream divergence.
    pub branch: BranchState,
    /// Dirty paths, including untracked files and renames.
    pub files: Vec<FileChange>,
    /// Recent commits, newest first.
    pub commits: Vec<CommitSummary>,
}

/// A workspace refresh result suitable for best-effort multi-repository views.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceReport {
    /// Configured workspace root.
    pub root: PathBuf,
    /// Snapshot when the path is a readable repository.
    pub snapshot: Option<WorkspaceSnapshot>,
    /// Human-readable typed error when inspection failed.
    pub error: Option<String>,
}

impl WorkspaceReport {
    /// Convert a single inspection result into a non-failing UI report.
    pub fn from_result(root: PathBuf, result: Result<WorkspaceSnapshot, WorkspaceError>) -> Self {
        match result {
            Ok(snapshot) => Self {
                root,
                snapshot: Some(snapshot),
                error: None,
            },
            Err(error) => Self {
                root,
                snapshot: None,
                error: Some(error.to_string()),
            },
        }
    }
}
