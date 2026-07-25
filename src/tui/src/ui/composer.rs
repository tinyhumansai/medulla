//! Composer text math. The draft is one string with embedded newlines; these
//! helpers map the flat caret offset onto rendered rows so the input box can
//! draw a multi-line draft with the caret on the correct row. Offsets are in
//! Unicode scalar values (chars), matching the JS string-index semantics closely
//! enough for terminal editing.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caret {
    pub row: usize,
    pub col: usize,
}

/// A composer draft: the text plus the caret's flat char offset into it. The
/// two travel together — an edit moving one without the other would strand the
/// caret on a character the user did not type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Draft {
    pub text: String,
    pub cursor: usize,
}

impl Draft {
    pub fn new() -> Self {
        Draft::default()
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
#[path = "composer_tests.rs"]
mod tests;
