//! Data types used while correlating GitHub PR commands with their results.

/// GitHub CLI operation whose output can authoritatively identify this PR.
#[derive(Clone, Copy)]
pub(crate) enum PullRequestCommand {
    /// `gh pr create` prints the newly created PR URL.
    Create,
    /// `gh pr view --json url` returns a structured URL property.
    View,
}

/// A correlated PR command and the logical checkout where it started.
pub(crate) struct PendingPullRequestCall {
    command: PullRequestCommand,
    workspace_cwd: Option<String>,
    workspace_branch: Option<String>,
}

impl PendingPullRequestCall {
    /// Bind a recognized command to the current tool-reported checkout.
    pub(crate) fn new(
        command: PullRequestCommand,
        workspace_cwd: Option<&str>,
        workspace_branch: Option<&str>,
    ) -> Self {
        Self {
            command,
            workspace_cwd: workspace_cwd.map(str::to_string),
            workspace_branch: workspace_branch.map(str::to_string),
        }
    }

    /// Return the command only if the session has not moved since it started.
    pub(crate) fn command_in(
        self,
        workspace_cwd: Option<&str>,
        workspace_branch: Option<&str>,
    ) -> Option<PullRequestCommand> {
        (self.workspace_cwd.as_deref() == workspace_cwd
            && self.workspace_branch.as_deref() == workspace_branch)
            .then_some(self.command)
    }
}
