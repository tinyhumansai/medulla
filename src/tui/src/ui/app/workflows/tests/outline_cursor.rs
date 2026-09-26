//! The outline cursor's own movement, independent of which layout draws it.
//!
//! A join sits on two outline rows that share one `node_index` — the subtree
//! it was first reached from, and the `revisit` row that references it back —
//! so `outline_cursor_row` cannot resolve the current row from the node alone
//! once the cursor is on the second one. These tests move onto and past a
//! join's `revisit` row and check the cursor lands where the move put it,
//! rather than snapping back to the first occurrence.

use super::{app_with, diamond};

#[test]
fn moving_onto_a_joins_revisit_row_keeps_the_cursor_there() {
    let (_home, mut app) = app_with(&[diamond("d")]);

    // `diamond`'s shape (see `app_with`'s fixture) walks: start, check, yes,
    // join (first occurrence), no, join (revisit) — six rows, with the join
    // node visited twice. See `rows_of` in `super::super::outline` for why the
    // `false` arm becomes the spine walked last.
    let rows = app.outline_rows();
    assert_eq!(rows.len(), 6, "{rows:?}");
    assert!(
        !rows[3].revisit,
        "the first join row is the subtree: {rows:?}"
    );
    assert_eq!(
        rows[3].node, rows[5].node,
        "both rows are the join: {rows:?}"
    );
    assert!(
        rows[5].revisit,
        "the second join row is the reference: {rows:?}"
    );

    for _ in 0..5 {
        app.move_outline_cursor(true);
    }

    let rows = app.outline_rows();
    assert_eq!(
        app.outline_cursor_row(&rows),
        5,
        "the cursor is on the row the move landed on, not the join's earlier row"
    );

    // Confirms the bug this guards: without a distinct row for the revisit,
    // resolving by node alone answers row 3 here, and a further `down` would
    // then move to row 4 (a row already behind the cursor) instead of staying
    // on the last row.
    app.move_outline_cursor(true);
    let rows = app.outline_rows();
    assert_eq!(
        app.outline_cursor_row(&rows),
        5,
        "there is nowhere further to go from the last row"
    );
}

#[test]
fn moving_off_a_joins_revisit_row_and_back_resolves_the_same_row() {
    let (_home, mut app) = app_with(&[diamond("d")]);

    for _ in 0..5 {
        app.move_outline_cursor(true);
    }
    let rows = app.outline_rows();
    assert_eq!(app.outline_cursor_row(&rows), 5);

    app.move_outline_cursor(false);
    let rows = app.outline_rows();
    assert_eq!(
        app.outline_cursor_row(&rows),
        4,
        "up from the revisit row is the row above it, not the join's other row"
    );
}
