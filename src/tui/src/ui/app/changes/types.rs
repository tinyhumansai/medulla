//! Data types for the session-scoped Git changes view.
//!
//! The review model itself — hunk boundaries, comment anchors, and the comment
//! store — lives in [`medulla::ui::git_review`] so surfaces other than the
//! terminal can reuse it. What stays here is the terminal session's cursor and
//! its cache of the last Git read.

use std::path::{Path, PathBuf};

use medulla::ui::git_review::{
    hunks, next_hunk, origin_label, previous_hunk, ChangeOrigin, CommentAnchor, Hunk,
    ReviewComments,
};

use super::repository;

/// One path changed relative to the session-start commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChangedFile {
    /// Git status code such as `M`, `A`, or `D`.
    pub(crate) status: String,
    /// Repository-relative path.
    pub(crate) path: PathBuf,
    /// Where the change currently lives: committed, staged, unstaged, untracked.
    pub(crate) origins: Vec<ChangeOrigin>,
}

impl ChangedFile {
    /// Combined origin label such as `commit+unstaged`, empty when unclassified.
    pub(crate) fn origin_label(&self) -> String {
        origin_label(&self.origins)
    }
}

/// Cached repository data and navigation for the Changes tab.
#[derive(Debug, Default)]
pub(crate) struct GitChangesState {
    /// Repository root discovered when the TUI session starts.
    pub(crate) root: Option<PathBuf>,
    /// Commit checked out when the TUI session starts.
    pub(crate) baseline: Option<String>,
    /// Commits made after the baseline, newest first.
    pub(crate) commits: Vec<String>,
    /// Paths changed by commits or working-tree edits since the baseline.
    pub(crate) files: Vec<ChangedFile>,
    /// Highlighted file.
    pub(crate) selected: usize,
    /// Highlighted line within the selected patch; the anchor for comments.
    pub(crate) cursor: usize,
    /// Hunk boundaries of the selected patch, in patch-line indices.
    pub(crate) hunks: Vec<Hunk>,
    /// Vertical offset into the selected patch.
    pub(crate) scroll: usize,
    /// Patch text for the selected file.
    pub(crate) patch: Vec<String>,
    /// Review notes kept for the lifetime of this TUI session.
    pub(crate) comments: ReviewComments,
    /// Largest valid vertical offset for the currently rendered detail pane.
    pub(crate) max_scroll: usize,
    /// Last repository error, rendered instead of an empty view.
    pub(crate) error: Option<String>,
}

impl GitChangesState {
    /// Capture the current repository and HEAD as the immutable session baseline.
    pub(crate) fn capture() -> Self {
        match repository::discover() {
            Ok((root, baseline)) => {
                let mut state = Self {
                    root: Some(root),
                    baseline: Some(baseline),
                    ..Self::default()
                };
                state.refresh();
                state
            }
            Err(error) => Self {
                error: Some(error),
                ..Self::default()
            },
        }
    }

    /// Reload repository data without changing the captured baseline or comments.
    pub(crate) fn refresh(&mut self) {
        let (Some(root), Some(baseline)) = (&self.root, &self.baseline) else {
            return;
        };
        match repository::load(root, baseline) {
            Ok((commits, files)) => {
                self.commits = commits;
                self.files = files;
                self.selected = self.selected.min(self.files.len().saturating_sub(1));
                self.error = None;
                self.reload_patch();
            }
            Err(error) => self.error = Some(error),
        }
    }

    /// Reload only the selected file's patch, re-indexing its hunks.
    pub(crate) fn reload_patch(&mut self) {
        self.scroll = 0;
        self.cursor = 0;
        let selected_path = self.selected_path().map(|p| p.to_path_buf());
        let new_patch = match (&self.root, &self.baseline, &selected_path) {
            (Some(root), Some(baseline), Some(path)) => {
                match repository::patch(root, baseline, path) {
                    Ok(p) => p,
                    Err(error) => {
                        // When patch loading fails, preserve the current patch and error state.
                        // Do not mark anchors outdated on transient failures or network issues.
                        self.error = Some(error);
                        return;
                    }
                }
            }
            _ => Vec::new(),
        };

        // Only mark anchors outdated if the patch has actually changed.
        // Comparing old and new patch content avoids spurious invalidation during
        // navigation or refresh without actual changes.
        let patch_changed = self.patch != new_patch;
        self.patch = new_patch;
        self.hunks = hunks(&self.patch);
        self.error = None;

        if patch_changed {
            if let Some(path) = selected_path {
                self.comments
                    .mark_outdated_if_invalid(&path, &self.patch, self.hunks.len());
            }
        }
    }

    /// Move the file selection, reloading the patch that comes into view.
    pub(crate) fn select_file(&mut self, delta: isize) {
        let last = self.files.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
        self.reload_patch();
    }

    /// Move the line cursor within the selected patch, clamped to its bounds.
    pub(crate) fn move_cursor(&mut self, delta: isize) {
        let last = self.patch.len().saturating_sub(1);
        self.cursor = self.cursor.saturating_add_signed(delta).min(last);
    }

    /// Jump the cursor to the next or previous hunk header.
    ///
    /// Returns `false` when there is no hunk in that direction, leaving the
    /// cursor where it is rather than wrapping past the end of the patch.
    pub(crate) fn jump_hunk(&mut self, forward: bool) -> bool {
        let target = if forward {
            next_hunk(&self.hunks, self.cursor)
        } else {
            previous_hunk(&self.hunks, self.cursor)
        };
        match target {
            Some(line) => {
                self.cursor = line;
                true
            }
            None => false,
        }
    }

    /// Index of the hunk containing the cursor, if the cursor is inside one.
    pub(crate) fn hunk_at_cursor(&self) -> Option<usize> {
        self.hunks
            .iter()
            .position(|hunk| hunk.contains(self.cursor))
    }

    /// The anchor the cursor currently addresses.
    ///
    /// A cursor resting on a `@@` header comments on the whole hunk; anywhere
    /// else inside a patch comments on that single line; a file with no patch
    /// at all can still take a file-level note.
    pub(crate) fn cursor_anchor(&self) -> CommentAnchor {
        if self.patch.is_empty() {
            return CommentAnchor::File;
        }
        match self.hunk_at_cursor() {
            Some(index) if self.hunks[index].header == self.cursor => CommentAnchor::Hunk(index),
            _ => CommentAnchor::Line(self.cursor),
        }
    }

    /// Repository-relative selected path, if any.
    pub(crate) fn selected_path(&self) -> Option<&Path> {
        self.files
            .get(self.selected)
            .map(|file| file.path.as_path())
    }

    /// Concise status-line summary for a successful or failed refresh.
    pub(crate) fn status_message(&self) -> String {
        self.error.clone().unwrap_or_else(|| {
            format!(
                "Changes since session start · {} commit(s) · {} file(s) · {} comment(s)",
                self.commits.len(),
                self.files.len(),
                self.comments.len()
            )
        })
    }
}
