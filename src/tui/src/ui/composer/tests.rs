//! Tests for the composer module.

use super::*;
use crossterm::event::{KeyCode, KeyEvent};

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

/// The wrapped rows as `(start, text)` pairs, which is all these tests assert.
fn wrapped(text: &str, width: usize) -> Vec<(usize, String)> {
    wrap_rows(text, width)
        .into_iter()
        .map(|row| (row.start, row.text))
        .collect()
}

#[test]
fn wrapping_breaks_after_the_last_space_that_fits() {
    assert_eq!(
        wrapped("alpha bravo charlie", 10),
        vec![
            (0, "alpha ".to_string()),
            (6, "bravo ".to_string()),
            (12, "charlie".to_string()),
        ]
    );
}

#[test]
fn wrapping_keeps_every_char_so_caret_offsets_stay_true() {
    // The property the caret depends on: rows tile the draft in order, and only
    // the hard newlines belong to no row.
    for (text, width) in [
        ("alpha bravo charlie", 10),
        ("one\ntwo\nthree", 4),
        ("  leading and trailing  ", 7),
        ("", 5),
        ("\n\n", 5),
        ("no-spaces-at-all-here", 3),
    ] {
        let rows = wrap_rows(text, width);
        let newlines = text.matches('\n').count();
        let covered: usize = rows.iter().map(|row| row.text.chars().count()).sum();
        assert_eq!(covered + newlines, text.chars().count(), "{text:?}");
        let mut next = 0;
        for row in &rows {
            assert!(row.start >= next, "{text:?} rows overlap or reorder");
            next = row.start + row.text.chars().count();
        }
    }
}

#[test]
fn an_unbreakable_run_is_cut_rather_than_looping_forever() {
    assert_eq!(
        wrapped("abcdefg", 3),
        vec![
            (0, "abc".to_string()),
            (3, "def".to_string()),
            (6, "g".to_string()),
        ]
    );
    // A zero width would divide the text into nothing; it is treated as one.
    assert_eq!(wrap_rows("ab", 0).len(), 2);
}

#[test]
fn hard_newlines_break_and_empty_lines_keep_a_row() {
    assert_eq!(
        wrapped("a\n\nb", 10),
        vec![
            (0, "a".to_string()),
            (2, String::new()),
            (3, "b".to_string()),
        ]
    );
}

#[test]
fn the_caret_lands_on_the_wrapped_row_it_belongs_to() {
    let rows = wrap_rows("alpha bravo charlie", 10);
    assert_eq!(caret_visual(&rows, 0), (0, 0));
    assert_eq!(caret_visual(&rows, 5), (0, 5));
    // Offset 6 is the first char of the second row, not one past the first.
    assert_eq!(caret_visual(&rows, 6), (1, 0));
    assert_eq!(caret_visual(&rows, 12), (2, 0));
    // Past the end clamps onto the trailing space of the last row.
    assert_eq!(caret_visual(&rows, 999), (2, 7));
}

#[test]
fn a_caret_on_a_hard_newline_stays_on_the_row_it_ends() {
    // Offset 3 sits on the `\n`, which belongs to no row: the operator is at the
    // end of "one", not at the start of "two".
    let rows = wrap_rows("one\ntwo", 10);
    assert_eq!(caret_visual(&rows, 3), (0, 3));
    assert_eq!(caret_visual(&rows, 4), (1, 0));
    // No rows at all cannot happen from `wrap_rows`, but the lookup still has to
    // answer rather than index into nothing.
    assert_eq!(caret_visual(&[], 7), (0, 0));
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

#[test]
fn text_prompt_edits_unicode_and_reports_terminal_actions() {
    let mut prompt = TextPrompt::new((), "title");
    assert_eq!(
        edit_prompt(&mut prompt, KeyEvent::from(KeyCode::Char('é'))),
        PromptAction::Editing
    );
    assert_eq!(prompt.draft.text, "é");
    assert_eq!(prompt.draft.cursor, 1);
    assert_eq!(
        edit_prompt(&mut prompt, KeyEvent::from(KeyCode::Enter)),
        PromptAction::Submit
    );
    assert_eq!(
        edit_prompt(&mut prompt, KeyEvent::from(KeyCode::Esc)),
        PromptAction::Cancel
    );
}

#[test]
fn prefilled_prompt_places_the_caret_after_unicode_text() {
    let prompt = TextPrompt::with_text("edit", "Worker label", "café");

    assert_eq!(prompt.kind, "edit");
    assert_eq!(prompt.title, "Worker label");
    assert_eq!(prompt.draft.text, "café");
    assert_eq!(prompt.draft.cursor, 4);
}
