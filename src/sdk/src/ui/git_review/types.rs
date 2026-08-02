//! Value types for hunks, change origins, and anchored review comments.

use std::fmt;
use std::path::PathBuf;

/// One unified-diff hunk located inside a patch's line list.
///
/// Indices address the patch exactly as it is displayed, so a cursor position
/// and a hunk boundary are directly comparable without re-parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// Index of the `@@ … @@` header line within the patch.
    pub header: usize,
    /// Index one past the hunk's last line.
    pub end: usize,
    /// The header line's text, used as a short label.
    pub label: String,
}

impl Hunk {
    /// Whether `line` falls inside this hunk, header included.
    pub fn contains(&self, line: usize) -> bool {
        line >= self.header && line < self.end
    }
}

/// Which part of the repository a path's change currently lives in.
///
/// A single path can carry several of these at once — for example a file that
/// was committed during the session and then edited again without staging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChangeOrigin {
    /// Changed by a commit made after the session baseline.
    Committed,
    /// Changed in the index relative to `HEAD`.
    Staged,
    /// Changed in the working tree relative to the index.
    Unstaged,
    /// Present in the working tree and not tracked by Git.
    Untracked,
}

impl ChangeOrigin {
    /// Short label suitable for a narrow file rail.
    pub fn label(self) -> &'static str {
        match self {
            Self::Committed => "commit",
            Self::Staged => "staged",
            Self::Unstaged => "unstaged",
            Self::Untracked => "untracked",
        }
    }
}

impl fmt::Display for ChangeOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// Render a path's origins as one `commit+unstaged` style label.
///
/// Returns an empty string when nothing is known about the path, which keeps
/// the caller from having to special-case an unclassified file.
pub fn origin_label(origins: &[ChangeOrigin]) -> String {
    origins
        .iter()
        .map(|origin| origin.label())
        .collect::<Vec<_>>()
        .join("+")
}

/// What a review comment is bound to within one file's patch.
///
/// Anchors are compared by value, so re-selecting the same position finds the
/// existing comment and edits it rather than stacking a second one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentAnchor {
    /// The file as a whole, independent of any patch position.
    File,
    /// The hunk at this index within the file's [`Hunk`] list.
    Hunk(usize),
    /// A single line, addressed by its zero-based index into the patch.
    Line(usize),
}

impl CommentAnchor {
    /// Human-readable description used in prompts and comment headers.
    pub fn describe(self) -> String {
        match self {
            Self::File => "file".into(),
            Self::Hunk(index) => format!("hunk {}", index + 1),
            Self::Line(index) => format!("line {}", index + 1),
        }
    }
}

/// A review note bound to a position in the session's change set.
///
/// Comments live only in memory for the lifetime of the session and are never
/// written back into the repository, so adding one cannot modify the working
/// tree. When a patch refreshes and an anchor can no longer be resolved, the
/// comment is marked outdated but retained so the user's written note is not lost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewComment {
    /// Repository-relative path the comment belongs to.
    pub path: PathBuf,
    /// Position within that file's patch.
    pub anchor: CommentAnchor,
    /// The operator's note.
    pub body: String,
    /// Whether this comment's anchor is no longer valid (patch changed, target disappeared).
    /// Outdated comments are displayed but cannot be edited and do not contribute to new anchors.
    pub outdated: bool,
    /// Original content at the anchored position (line text or hunk header).
    /// Used to detect when line/hunk indices shift due to insertions/deletions.
    pub anchor_context: String,
}
