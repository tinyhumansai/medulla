//! Data types for the Git checkout a session or a diff is pointed at.

/// Which repository, which worktree, and which branch a directory sits in.
///
/// One value answers "where is this session working" in the vocabulary an
/// operator running several worktrees of one repository actually uses. Every
/// field is optional because every one of them can be genuinely absent: a
/// directory outside Git has none, the primary checkout has no worktree name,
/// and a detached or unborn `HEAD` has no branch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Checkout {
    /// The repository's directory name — `medulla-public`, not its remote URL.
    ///
    /// Read from the *common* Git directory, so every linked worktree of one
    /// repository reports the same name rather than the name of its own folder.
    pub repo: Option<String>,
    /// The linked worktree's name, or `None` in the repository's primary
    /// checkout.
    ///
    /// `None` is the meaningful default rather than a missing value: the
    /// primary checkout has no worktree name to show, and a status line that
    /// invented one for it would put the same word on every row.
    ///
    /// This is the name *Git* registered — the directory the worktree was
    /// created in, deduplicated if that name was taken — and deliberately not
    /// the branch checked out in it, which is [`branch`](Self::branch) and
    /// moves independently.
    pub worktree: Option<String>,
    /// The checked-out branch, or `None` on a detached `HEAD`.
    pub branch: Option<String>,
    /// The abbreviated commit `HEAD` resolves to, when there is one.
    ///
    /// Present alongside `branch` rather than instead of it, so a detached
    /// checkout still has something to name itself with — and so a caller that
    /// wants the commit does not have to ask Git a second time.
    pub head: Option<String>,
}

impl Checkout {
    /// Whether this directory is inside a repository at all.
    ///
    /// A checkout is known by having a repository name: the branch, the
    /// worktree and the commit can each be legitimately absent inside a
    /// perfectly ordinary repository, and only this one cannot.
    pub fn is_repository(&self) -> bool {
        self.repo.is_some()
    }

    /// Whether this is a linked worktree rather than the primary checkout.
    pub fn is_worktree(&self) -> bool {
        self.worktree.is_some()
    }

    /// How the branch is spelled in a status line.
    ///
    /// A detached `HEAD` is spelled `@a1b2c3d` — the `@` is what distinguishes
    /// it from a branch whose name happens to look like a hash, and showing
    /// nothing at all (which is what a branch-only field did) leaves an
    /// operator on a detached checkout with no way to tell it apart from a
    /// directory outside Git.
    pub fn branch_label(&self) -> Option<String> {
        match (&self.branch, &self.head) {
            (Some(branch), _) => Some(branch.clone()),
            (None, Some(head)) => Some(format!("@{head}")),
            (None, None) => None,
        }
    }

    /// A one-line description for a panel title, or `None` outside a repository.
    ///
    /// Reads `repo ⑂ worktree · branch`, dropping whichever parts are absent.
    /// The worktree glyph is deliberately not a word: the string sits in a
    /// panel title next to several other facts, and "worktree" spelled out is
    /// wider than the name it labels.
    pub fn summary(&self) -> Option<String> {
        let repo = self.repo.as_deref()?;
        let mut text = repo.to_owned();
        if let Some(worktree) = &self.worktree {
            text.push_str(" ⑂ ");
            text.push_str(worktree);
        }
        if let Some(branch) = self.branch_label() {
            text.push_str(" · ");
            text.push_str(&branch);
        }
        Some(text)
    }
}
