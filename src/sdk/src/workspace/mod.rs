//! Local Git workspace inspection for operator-facing workflow tools.
//!
//! The module deliberately wraps the `git` executable with argument vectors
//! instead of a shell. It is read-only: commit/staging operations live in a
//! [`commit`] adds a guarded write path that stages and commits only explicitly
//! named repository-relative paths.

mod commit;
mod git;
mod types;

#[cfg(test)]
mod tests;

pub use commit::commit;
pub use git::{
    current_branch, diff, diff_name_only, inspect_workspace, log_recent, status_porcelain,
};
pub use types::{
    BranchState, CommitError, CommitOptions, CommitOutcome, CommitSummary, FileChange,
    WorkspaceError, WorkspaceReport, WorkspaceSnapshot,
};
