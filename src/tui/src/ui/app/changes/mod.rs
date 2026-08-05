//! Session-scoped Git inspection and review comments for the Changes tab.
//!
//! Git subprocesses live in [`repository`], the cursor and cache in [`types`],
//! and the reusable review model in [`medulla::ui::git_review`]. This module is
//! only the bridge: it turns an operator's request to comment into a prompt
//! bound to the position the cursor is on.

mod baseline;
mod repository;
/// Visible to the rest of `ui::app` so the render layer and its tests can name
/// the row type they draw; the state itself is re-exported below.
pub(super) mod types;

#[cfg(test)]
mod baseline_tests;
#[cfg(test)]
mod comment_tests;
#[cfg(test)]
mod tests;

use std::path::Path;

use medulla::ui::git_review::CommentAnchor;
pub(crate) use types::GitChangesState;

use super::types::{App, Cmd, PromptKind, TABS};
use crate::ui::composer::{Draft, TextPrompt};
use baseline::select_harness_baseline;

impl App {
    /// Open the Changes tab for the harness currently shown in the Agents pane.
    ///
    /// The draw path records the exact session resolved from the selected rail
    /// row. Keeping that id before changing tabs lets the forced refresh pick
    /// the matching launch snapshot instead of falling back to another, newer
    /// harness in a different checkout. Unlike ordinary tab entry, this also
    /// replaces an operator-selected commit or manual baseline: `d` means the
    /// immutable launch diff for the harness under the cursor.
    pub(super) fn open_selected_harness_changes(&mut self) -> Option<Cmd> {
        let session = self.harness_pane_session.clone()?;
        self.selected_harness_session = Some(session);
        self.tab_index = TABS
            .iter()
            .position(|tab| *tab == "Changes")
            .expect("Changes is a top-level tab");
        self.selected = 0;
        self.refresh_changes_from_selected_harness();
        None
    }

    /// Reload commits, changed paths, and the selected patch from Git.
    pub(super) fn refresh_changes(&mut self) {
        self.refresh_changes_with_harness(false);
    }

    /// Reload Changes and activate the selected harness's launch snapshot.
    fn refresh_changes_from_selected_harness(&mut self) {
        self.refresh_changes_with_harness(true);
    }

    /// Reload Changes, optionally overriding an operator-selected baseline.
    fn refresh_changes_with_harness(&mut self, activate_harness: bool) {
        let preferred_id = self
            .attached_harness()
            .map(str::to_owned)
            .or_else(|| self.selected_harness_session.clone());
        let selected = self.harnesses.as_ref().map(|harnesses| {
            select_harness_baseline(harnesses.sessions.rows(), preferred_id.as_deref())
        });
        match selected {
            Some(Ok(Some((row, commit)))) => {
                let root = row.launch_root.as_deref().unwrap_or(&row.cwd);
                self.changes.follow_harness(
                    Path::new(root),
                    &commit,
                    row.launch_checkout_identity
                        .as_deref()
                        .expect("validated harness identity"),
                );
                if activate_harness {
                    if let Err(error) = self.changes.choose_harness_baseline() {
                        self.set_status(error);
                    } else {
                        self.set_status(self.changes.status_message());
                    }
                    return;
                }
            }
            Some(Err(error)) => {
                self.changes.clear_repository(error);
                self.set_status(self.changes.status_message());
                return;
            }
            _ => {}
        }
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
