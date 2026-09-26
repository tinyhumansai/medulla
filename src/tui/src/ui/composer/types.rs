//! Data types for the `composer` module.
#[allow(unused_imports)]
use super::*;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caret {
    pub row: usize,
    pub col: usize,
}
/// One rendered row of a soft-wrapped draft.
///
/// `start` is the flat char offset of the row's first char in the whole draft,
/// so a caret offset can be turned back into a (row, column) without re-walking
/// the text. Rows tile the draft in order and never overlap; hard newlines are
/// the only chars that belong to no row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisualRow {
    /// Flat char offset of this row's first char within the draft.
    pub start: usize,
    /// The row's chars, exactly as they should be drawn.
    pub text: String,
}

/// A composer draft: the text plus the caret's flat char offset into it. The
/// two travel together — an edit moving one without the other would strand the
/// caret on a character the user did not type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Draft {
    pub text: String,
    pub cursor: usize,
}

/// A domain-tagged single-line text prompt.
///
/// `K` describes what submitting the text means; editing and rendering remain
/// shared regardless of whether the caller is the main or daemon TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextPrompt<K> {
    /// Domain action performed when the prompt is submitted.
    pub kind: K,
    /// Human-facing panel title.
    pub title: String,
    /// Editable text and caret.
    pub draft: Draft,
}

impl<K> TextPrompt<K> {
    /// Create an empty prompt for `kind`.
    pub fn new(kind: K, title: impl Into<String>) -> Self {
        Self {
            kind,
            title: title.into(),
            draft: Draft::new(),
        }
    }

    /// Create a prompt whose caret starts after `text`.
    pub fn with_text(kind: K, title: impl Into<String>, text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            kind,
            title: title.into(),
            draft: Draft {
                cursor: text.chars().count(),
                text,
            },
        }
    }
}

/// Result of routing a key through [`super::edit_prompt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptAction {
    /// The key was consumed as an edit or intentionally ignored.
    Editing,
    /// The caller should close the prompt without submitting.
    Cancel,
    /// The caller should consume and submit the prompt.
    Submit,
}
