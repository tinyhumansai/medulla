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
mod tests {
    use super::*;

    #[test]
    fn caret_maps_rows_and_cols() {
        let text = "ab\ncde\nf";
        assert_eq!(caret_row_col(text, 0), Caret { row: 0, col: 0 });
        assert_eq!(caret_row_col(text, 2), Caret { row: 0, col: 2 });
        assert_eq!(caret_row_col(text, 3), Caret { row: 1, col: 0 });
        assert_eq!(caret_row_col(text, 6), Caret { row: 1, col: 3 });
        assert_eq!(caret_row_col(text, 7), Caret { row: 2, col: 0 });
        // Clamps past the end.
        assert_eq!(caret_row_col(text, 999), Caret { row: 2, col: 1 });
    }

    #[test]
    fn insert_advances_caret() {
        let d = insert_at("hello", 5, "!");
        assert_eq!(d.text, "hello!");
        assert_eq!(d.cursor, 6);
        let mid = insert_at("hello", 2, "XY");
        assert_eq!(mid.text, "heXYllo");
        assert_eq!(mid.cursor, 4);
    }

    #[test]
    fn insert_newline_multiline() {
        let d = insert_at("ab", 1, "\n");
        assert_eq!(d.text, "a\nb");
        assert_eq!(d.cursor, 2);
    }

    #[test]
    fn move_row_keeps_column() {
        let text = "abcd\nef";
        // caret at row 0 col 3, move down → row 1 clamps to col 2.
        assert_eq!(move_caret_row(text, 3, 1), Some(5 + 2));
        // at first row moving up → None (history recall).
        assert_eq!(move_caret_row(text, 3, -1), None);
        // at last row moving down → None.
        assert_eq!(move_caret_row(text, 6, 1), None);
    }

    #[test]
    fn delete_before_steps_back() {
        let d = delete_before("hello", 5);
        assert_eq!(d.text, "hell");
        assert_eq!(d.cursor, 4);
        assert_eq!(delete_before("x", 0).cursor, 0);
    }

    #[test]
    fn unicode_offsets() {
        let d = insert_at("héllo", 2, "!");
        assert_eq!(d.text, "hé!llo");
        assert_eq!(d.cursor, 3);
    }
}
