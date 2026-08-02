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

#[test]
fn paste_normalisation_collapses_every_line_ending_to_lf() {
    assert_eq!(normalize_paste("a\r\nb\rc\nd"), "a\nb\nc\nd");
    // A CRLF must not become two newlines by way of the bare-CR pass.
    assert_eq!(normalize_paste("a\r\n\r\nb"), "a\n\nb");
    assert_eq!(normalize_paste("plain"), "plain");
}

#[test]
fn flattening_turns_line_breaks_into_spaces_for_single_line_fields() {
    assert_eq!(flatten_paste("token\r\n"), "token ");
    assert_eq!(flatten_paste("a\rb"), "a b");
    // The trailing space is what every caller's `trim` on submit removes.
    assert_eq!(flatten_paste("token\n").trim(), "token");
}

#[test]
fn prompt_paste_flattens_at_the_caret_and_advances_it() {
    let mut prompt = TextPrompt::with_text((), "title", "ac");
    prompt.draft.cursor = 1;

    prompt.paste("b\nX");

    assert_eq!(prompt.draft.text, "ab Xc");
    assert_eq!(prompt.draft.cursor, 4);
}
