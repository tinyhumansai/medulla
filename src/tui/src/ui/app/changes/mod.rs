//! Session-scoped Git inspection and review comments for the Changes tab.
//!
//! Git subprocesses live in [`repository`], the cursor and cache in [`types`],
//! and the reusable review model in [`medulla::ui::git_review`]. This module is
//! only the bridge: it turns an operator's request to comment into a prompt
//! bound to the position the cursor is on.

mod repository;
/// Visible to the rest of `ui::app` so the render layer and its tests can name
/// the row type they draw; the state itself is re-exported below.
pub(super) mod types;

#[cfg(test)]
mod comment_tests;
#[cfg(test)]
mod tests;

use std::path::Path;

use medulla::ui::git_review::CommentAnchor;
pub(crate) use types::GitChangesState;

use crate::ui::composer::{Draft, TextPrompt};

use super::types::{App, PromptKind};
use types::BaselineSource;

impl App {
    /// Reload commits, changed paths, and the selected patch from Git.
    pub(super) fn refresh_changes(&mut self) {
        let preferred_id = self
            .attached_harness()
            .map(str::to_owned)
            .or_else(|| self.selected_harness_session.clone());
        let latest = self.harnesses.as_ref().and_then(|harnesses| {
            let rows: Vec<_> = harnesses
                .sessions
                .rows()
                .into_iter()
                .filter(|row| row.launch_commit.is_some())
                .collect();
            preferred_id
                .as_deref()
                .and_then(|id| rows.iter().find(|row| row.id == id).cloned())
                .or_else(|| rows.into_iter().max_by_key(|row| row.started_at))
                .and_then(|row| row.launch_commit.clone().map(|commit| (row, commit)))
        });
        if let Some((row, commit)) = latest {
            self.changes.follow_harness(Path::new(&row.cwd), &commit);
        }
        self.changes.refresh();
        self.set_status(self.changes.status_message());
    }

    /// Open the baseline selector over the launch snapshot and recent history.
    pub(super) fn open_change_baseline_picker(&mut self) {
        self.changes.picking_baseline = true;
        self.changes.baseline_index = 0;
        self.set_status("Baseline: ↑/↓ select · Enter apply · m manual · Esc cancel");
    }

    /// Apply the highlighted baseline option, opening manual input when needed.
    pub(super) fn apply_change_baseline_selection(&mut self) {
        let index = self.changes.baseline_index;
        if index == 0 {
            match self.changes.choose_harness_baseline() {
                Ok(()) => self.set_status(self.changes.status_message()),
                Err(error) => self.set_status(error),
            }
        } else if let Some(commit) = self.changes.recent_commits.get(index - 1) {
            self.finish_change_baseline(&commit.id.clone(), BaselineSource::Commit);
        } else {
            self.open_manual_change_baseline();
        }
    }

    /// Ask for an arbitrary commit id or revision.
    pub(super) fn open_manual_change_baseline(&mut self) {
        self.changes.picking_baseline = false;
        self.prompt = Some(TextPrompt {
            title: "Diff from commit or revision".to_owned(),
            draft: Draft::new(),
            kind: PromptKind::ChangesBaseline,
        });
        self.set_status("Enter a commit id or revision · Enter apply · Esc cancel");
    }

    /// Validate and activate a requested baseline, preserving errors in status.
    pub(super) fn finish_change_baseline(&mut self, revision: &str, source: BaselineSource) {
        match self.changes.choose_baseline(revision, source) {
            Ok(()) => self.set_status(self.changes.status_message()),
            Err(error) => self.set_status(error),
        }
    }

    /// Open a comment prompt bound to the line or hunk under the cursor.
    pub(super) fn comment_on_change(&mut self) {
        let anchor = self.changes.cursor_anchor();
        self.open_change_comment(anchor);
    }

    /// Open a comment prompt bound to the selected file as a whole.
    pub(super) fn comment_on_change_file(&mut self) {
        let existing = self
            .changes
            .selected_path()
            .and_then(|path| self.changes.comments.body(path, CommentAnchor::File))
            .map(str::to_owned);
        match existing {
            Some(body) => self.open_change_comment_with(CommentAnchor::File, body),
            None => self.open_change_comment(CommentAnchor::File),
        }
    }

    /// Open a comment prompt seeded with the note already at the cursor.
    ///
    /// Editing and adding are the same prompt; submitting empty text removes the
    /// note. Nothing on this path touches the working tree. Outdated comments
    /// cannot be edited since their anchors have drifted; warn and refuse.
    pub(super) fn edit_change_comment(&mut self) {
        let anchor = self.changes.cursor_anchor();
        let path = match self.changes.selected_path() {
            Some(p) => p,
            None => {
                self.set_status("No changed file selected");
                return;
            }
        };

        // Find the comment and check if it's outdated.
        let comment = self
            .changes
            .comments
            .for_path(path)
            .find(|c| c.anchor == anchor);

        match comment {
            Some(c) if c.outdated => self.set_status(
                "This comment anchor is outdated (content drifted) · delete and re-anchor it",
            ),
            Some(c) => self.open_change_comment_with(anchor, c.body.clone()),
            None => self.set_status("No comment here yet · press c to add one"),
        }
    }

    /// Open an empty comment prompt for `anchor`.
    fn open_change_comment(&mut self, anchor: CommentAnchor) {
        self.open_change_comment_with(anchor, String::new());
    }

    /// Open a comment prompt for `anchor`, pre-filled with `body`.
    fn open_change_comment_with(&mut self, anchor: CommentAnchor, body: String) {
        let Some(path) = self.changes.selected_path().map(Path::to_path_buf) else {
            self.set_status("No changed file selected");
            return;
        };
        let cursor = body.chars().count();
        self.prompt = Some(TextPrompt {
            title: format!(
                "Comment on {} · {}",
                path.display(),
                anchor_title(anchor, &self.changes)
            ),
            draft: Draft { text: body, cursor },
            kind: PromptKind::ChangesComment { path, anchor },
        });
    }
}

/// Describe an anchor for the prompt title, naming the hunk when there is one.
fn anchor_title(anchor: CommentAnchor, changes: &GitChangesState) -> String {
    match anchor {
        CommentAnchor::Hunk(index) => changes
            .hunks
            .get(index)
            .map_or_else(|| anchor.describe(), |hunk| hunk.label.clone()),
        other => other.describe(),
    }
}
