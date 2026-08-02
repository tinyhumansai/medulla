//! The session's in-memory store of anchored review comments.
//!
//! The store is deliberately additive-and-editable rather than append-only: an
//! operator who mistypes a note re-selects the same anchor and overwrites it,
//! and clearing the text removes the comment. Nothing here writes to disk, so a
//! comment can never modify the reviewed working tree.

use std::path::Path;

use super::types::{CommentAnchor, ReviewComment};

/// Review comments held for the lifetime of one TUI session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewComments {
    items: Vec<ReviewComment>,
}

impl ReviewComments {
    /// Add or replace the comment at `path` + `anchor`.
    ///
    /// An empty or whitespace-only `body` deletes any comment already stored
    /// there, which is how editing doubles as removal. Returns `true` when a
    /// comment now exists at that anchor.
    pub fn upsert(&mut self, path: &Path, anchor: CommentAnchor, body: &str) -> bool {
        let body = body.trim();
        let existing = self
            .items
            .iter()
            .position(|item| item.path == path && item.anchor == anchor);
        match (existing, body.is_empty()) {
            (Some(index), true) => {
                self.items.remove(index);
                false
            }
            (Some(index), false) => {
                self.items[index].body = body.to_owned();
                true
            }
            (None, true) => false,
            (None, false) => {
                self.items.push(ReviewComment {
                    path: path.to_path_buf(),
                    anchor,
                    body: body.to_owned(),
                    outdated: false,
                });
                true
            }
        }
    }

    /// The body stored at `path` + `anchor`, if any.
    pub fn body(&self, path: &Path, anchor: CommentAnchor) -> Option<&str> {
        self.items
            .iter()
            .find(|item| item.path == path && item.anchor == anchor)
            .map(|item| item.body.as_str())
    }

    /// Every comment on `path`, in the order they were first written.
    pub fn for_path<'a>(&'a self, path: &'a Path) -> impl Iterator<Item = &'a ReviewComment> + 'a {
        self.items.iter().filter(move |item| item.path == path)
    }

    /// How many comments are stored for `path`.
    pub fn count_for(&self, path: &Path) -> usize {
        self.for_path(path).count()
    }

    /// Total number of comments across every file.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether no comment has been written yet.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Every comment in the session, for reporting or hand-off.
    pub fn iter(&self) -> impl Iterator<Item = &ReviewComment> {
        self.items.iter()
    }

    /// Mark comments outdated when their anchors can no longer be resolved.
    ///
    /// When a patch refreshes, line and hunk indices may no longer point to valid
    /// positions. Rather than deleting a user's written comment, we mark it outdated.
    /// Outdated comments remain visible (so the user's work is not lost) but cannot be
    /// edited and do not participate in new anchor resolution. File-level comments are
    /// never marked outdated since they are position-independent.
    pub fn mark_outdated_if_invalid(&mut self, path: &Path, new_patch_len: usize) {
        for item in self.items.iter_mut() {
            if item.path != path {
                continue; // skip comments for other paths
            }
            if item.outdated {
                continue; // already marked
            }
            match item.anchor {
                CommentAnchor::File => {
                    // file-level comments always remain valid
                }
                CommentAnchor::Line(index) => {
                    if index >= new_patch_len {
                        // line index no longer valid after refresh
                        item.outdated = true;
                    }
                }
                CommentAnchor::Hunk(_) => {
                    // hunk indices are unstable across refreshes; mark all as outdated
                    item.outdated = true;
                }
            }
        }
    }
}
