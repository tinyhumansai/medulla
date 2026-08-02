//! Data types for the session-scoped Git changes view.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::repository;

/// One path changed relative to the session-start commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChangedFile {
    /// Git status code such as `M`, `A`, or `D`.
    pub(crate) status: String,
    /// Repository-relative path.
    pub(crate) path: String,
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
    /// Vertical offset into the selected patch.
    pub(crate) scroll: usize,
    /// Patch text for the selected file.
    pub(crate) patch: Vec<String>,
    /// Review notes kept for the lifetime of this TUI session.
    pub(crate) comments: BTreeMap<String, Vec<String>>,
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

    /// Reload only the selected file's patch.
    pub(crate) fn reload_patch(&mut self) {
        self.scroll = 0;
        self.patch = match (&self.root, &self.baseline, self.selected_path()) {
            (Some(root), Some(baseline), Some(path)) => {
                repository::patch(root, baseline, path).unwrap_or_else(|error| vec![error])
            }
            _ => Vec::new(),
        };
    }

    /// Repository-relative selected path, if any.
    pub(crate) fn selected_path(&self) -> Option<&str> {
        self.files.get(self.selected).map(|file| file.path.as_str())
    }

    /// Concise status-line summary for a successful or failed refresh.
    pub(crate) fn status_message(&self) -> String {
        self.error.clone().unwrap_or_else(|| {
            format!(
                "Changes since session start · {} commit(s) · {} file(s)",
                self.commits.len(),
                self.files.len()
            )
        })
    }
}
