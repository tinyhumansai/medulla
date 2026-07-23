//! Local Git workspace inspection for operator-facing workflow tools.
//!
//! The module deliberately wraps the `git` executable with argument vectors
//! instead of a shell. It is read-only: commit/staging operations live in a
//! separate milestone and build on these typed repository views.

mod git;
mod types;

#[cfg(test)]
mod tests;

pub use git::{
    current_branch, diff, diff_name_only, inspect_workspace, log_recent, status_porcelain,
};
pub use types::{
    BranchState, CommitSummary, FileChange, WorkspaceError, WorkspaceReport, WorkspaceSnapshot,
};
