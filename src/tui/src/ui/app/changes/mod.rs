//! Session-scoped Git inspection and review comments for the Changes tab.
//!
//! Git subprocesses live in [`repository`], the cursor and cache in [`types`],
//! and the reusable review model in [`medulla::ui::git_review`]. This module is
//! only the bridge: it turns an operator's request to comment into a prompt
//! bound to the position the cursor is on.

mod repository;
mod types;

#[cfg(test)]
mod tests;

use std::path::Path;

use medulla::ui::git_review::CommentAnchor;
pub(crate) use types::GitChangesState;

use crate::ui::composer::{Draft, TextPrompt};

use super::types::{App, PromptKind};

impl App {
    /// Reload commits, changed paths, and the selected patch from Git.
    pub(super) fn refresh_changes(&mut self) {
        self.changes.refresh();
        self.set_status(self.changes.status_message());
    }

    /// Open a comment prompt bound to the line or hunk under the cursor.
    pub(super) fn comment_on_change(&mut self) {
        let anchor = self.changes.cursor_anchor();
        self.open_change_comment(anchor);
    }

    /// Open a comment prompt bound to the selected file as a whole.
    pub(super) fn comment_on_change_file(&mut self) {
        self.open_change_comment(CommentAnchor::File);
    }

    /// Open a comment prompt seeded with the note already at the cursor.
    ///
    /// Editing and adding are the same prompt; submitting empty text removes the
    /// note. Nothing on this path touches the working tree.
    pub(super) fn edit_change_comment(&mut self) {
        let anchor = self.changes.cursor_anchor();
        let existing = self
            .changes
            .selected_path()
            .and_then(|path| self.changes.comments.body(path, anchor))
            .map(str::to_owned);
        match existing {
            Some(body) => self.open_change_comment_with(anchor, body),
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
