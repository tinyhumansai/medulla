//! Composer text math. The draft is one string with embedded newlines; these
//! helpers map the flat caret offset onto rendered rows so the input box can
//! draw a multi-line draft with the caret on the correct row. The shared
//! [`TextPrompt`] and [`edit_prompt`] path also keeps main and daemon overlays
//! behaviorally identical. Offsets are in Unicode scalar values (chars),
//! matching the JS string-index semantics closely enough for terminal editing.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl Draft {
    pub fn new() -> Self {
        Draft::default()
    }
}

impl<K> TextPrompt<K> {
    /// Insert a paste payload at the prompt's caret, flattened to one line.
    ///
    /// Prompts submit on `Enter`, so a payload that kept its line breaks would
    /// have to be either dropped or turned into several submissions; flattening
    /// keeps the whole paste visible and editable instead.
    pub fn paste(&mut self, text: &str) {
        self.draft = insert_at(&self.draft.text, self.draft.cursor, &flatten_paste(text));
    }
}

/// Apply standard single-line prompt editing.
///
/// Control and Alt character chords are consumed without inserting their
/// printable character; callers retain ownership of global chords before
/// invoking this helper.
pub fn edit_prompt<K>(prompt: &mut TextPrompt<K>, key: KeyEvent) -> PromptAction {
    match key.code {
        KeyCode::Esc => PromptAction::Cancel,
        KeyCode::Enter => PromptAction::Submit,
        KeyCode::Backspace | KeyCode::Delete => {
            prompt.draft = delete_before(&prompt.draft.text, prompt.draft.cursor);
            PromptAction::Editing
        }
        KeyCode::Left => {
            prompt.draft.cursor = prompt.draft.cursor.saturating_sub(1);
            PromptAction::Editing
        }
        KeyCode::Right => {
            prompt.draft.cursor = (prompt.draft.cursor + 1).min(prompt.draft.text.chars().count());
            PromptAction::Editing
        }
        KeyCode::Char(ch)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            prompt.draft = insert_at(&prompt.draft.text, prompt.draft.cursor, &ch.to_string());
            PromptAction::Editing
        }
        _ => PromptAction::Editing,
    }
}

fn char_len(text: &str) -> usize {
    text.chars().count()
}

/// Byte index for a char offset (clamped to text length).
fn byte_at(text: &str, char_offset: usize) -> usize {
    text.char_indices()
        .nth(char_offset)
        .map(|(b, _)| b)
        .unwrap_or(text.len())
}

/// Locate the caret's (row, col) within `text` for a flat `cursor` offset.
pub fn caret_row_col(text: &str, cursor: usize) -> Caret {
    let clamped = cursor.min(char_len(text));
    let before: String = text.chars().take(clamped).collect();
    let row = before.matches('\n').count();
    let col = match before.rfind('\n') {
        Some(idx) => clamped - (before[..=idx].chars().count()),
        None => clamped,
    };
    Caret { row, col }
}

/// Normalise a bracketed-paste payload to the composer's newline form.
///
/// A terminal hands the payload over verbatim, so text copied from a Windows
/// file carries `\r\n` and text from a classic-Mac source carries a bare `\r`.
/// The draft stores `\n` only, and the caret math here ([`caret_row_col`],
/// [`move_caret_row`]) counts `\n` alone — an embedded `\r` would leave the
/// caret pointing at a row the renderer never draws, and most terminals redraw
/// a stray `\r` as a jump to column zero.
pub fn normalize_paste(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Normalise a paste payload for a single-line field.
///
/// Line-oriented prompts (the login token box, the worker's pairing and
/// workspace prompts) have no second row to put a line break on, so every break
/// becomes a space. The common case — text copied with a trailing newline — then
/// lands as the value plus one space, which every caller's `trim` on submit
/// removes.
pub fn flatten_paste(text: &str) -> String {
    normalize_paste(text).replace('\n', " ")
}

/// Insert `value` at the caret, returning the new draft.
pub fn insert_at(text: &str, cursor: usize, value: &str) -> Draft {
    let clamped = cursor.min(char_len(text));
    let byte = byte_at(text, clamped);
    let mut out = String::with_capacity(text.len() + value.len());
    out.push_str(&text[..byte]);
    out.push_str(value);
    out.push_str(&text[byte..]);
    Draft {
        text: out,
        cursor: clamped + char_len(value),
    }
}

/// Move the caret one row up (`delta` -1) or down (+1), keeping its column where
/// the target row is long enough. Returns `None` when there is no such row — the
/// caller then falls through to prompt-history recall.
pub fn move_caret_row(text: &str, cursor: usize, delta: i32) -> Option<usize> {
    let rows: Vec<&str> = text.split('\n').collect();
    let Caret { row, col } = caret_row_col(text, cursor);
    let target = row as i32 + delta;
    if target < 0 || target as usize >= rows.len() {
        return None;
    }
    let target = target as usize;
    let mut start = 0usize;
    for r in rows.iter().take(target) {
        start += char_len(r) + 1;
    }
    Some(start + col.min(char_len(rows[target])))
}

/// Delete the char before the caret; returns the new draft (no-op at offset 0).
pub fn delete_before(text: &str, cursor: usize) -> Draft {
    if cursor == 0 {
        return Draft {
            text: text.to_string(),
            cursor: 0,
        };
    }
    let clamped = cursor.min(char_len(text));
    let prev = clamped - 1;
    let start = byte_at(text, prev);
    let end = byte_at(text, clamped);
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..start]);
    out.push_str(&text[end..]);
    Draft {
        text: out,
        cursor: prev,
    }
}

#[cfg(test)]
mod tests;

mod types;
pub use types::{Caret, Draft, PromptAction, TextPrompt, VisualRow};

mod wrap;
pub use wrap::{caret_visual, wrap_rows};
