//! The session's in-memory store of anchored review comments.
//!
//! The store is deliberately additive-and-editable rather than append-only: an
//! operator who mistypes a note re-selects the same anchor and overwrites it,
//! and clearing the text removes the comment. Nothing here writes to disk, so a
//! comment can never modify the reviewed working tree.

use std::path::Path;

use super::types::{CommentAnchor, Hunk, ReviewComment};

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
        self.upsert_with_context(path, anchor, body, "")
    }

    /// Add or replace the comment at `path` + `anchor`, capturing context for drift detection.
    ///
    /// The `context` is the original content at the anchored position (line text or hunk header).
    /// On refresh, this context is verified to detect when indices shift due to insertions/deletions.
    pub fn upsert_with_context(
        &mut self,
        path: &Path,
        anchor: CommentAnchor,
        body: &str,
        context: &str,
    ) -> bool {
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
                self.items[index].anchor_context = context.to_owned();
                self.items[index].outdated = false;
                true
            }
            (None, true) => false,
            (None, false) => {
                self.items.push(ReviewComment {
                    path: path.to_path_buf(),
                    anchor,
                    body: body.to_owned(),
                    outdated: false,
                    anchor_context: context.to_owned(),
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
    /// positions or content may have shifted. Rather than deleting a user's written
    /// comment, we mark it outdated. Outdated comments remain visible (so the user's
    /// work is not lost) but cannot be edited and do not participate in new anchor
    /// resolution. File-level comments are never marked outdated since they are
    /// position-independent.
    ///
    /// Line comments are marked outdated if:
    /// - Their index exceeds the patch length, OR
    /// - Their stored context (line text) no longer matches at that index
    ///
    /// Hunk comments are marked outdated if:
    /// - Their index exceeds the hunk count, OR
    /// - Their stored context (hunk header) no longer matches at that index
    pub fn mark_outdated_if_invalid(
        &mut self,
        path: &Path,
        patch_lines: &[String],
        hunks: &[Hunk],
    ) {
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
                    let is_valid = index < patch_lines.len()
                        && (item.anchor_context.is_empty()
                            || patch_lines[index] == item.anchor_context);
                    if !is_valid {
                        item.outdated = true;
                    }
                }
                CommentAnchor::Hunk(index) => {
                    let is_valid = index < hunks.len()
                        && (item.anchor_context.is_empty()
                            || patch_lines
                                .get(hunks[index].header)
                                .map_or(false, |line| line == &item.anchor_context));
                    if !is_valid {
                        item.outdated = true;
                    }
                }
            }
        }
    }
}
