//! Unit tests for hunk indexing, change-origin labels, and the comment store.

use std::path::Path;

use super::{
    hunks, next_hunk, origin_label, previous_hunk, ChangeOrigin, CommentAnchor, ReviewComments,
};

/// A two-hunk patch with the usual `diff --git` preamble.
fn patch() -> Vec<String> {
    [
        "diff --git a/src/lib.rs b/src/lib.rs",
        "--- a/src/lib.rs",
        "+++ b/src/lib.rs",
        "@@ -1,3 +1,4 @@",
        " one",
        "+two",
        "@@ -20,2 +21,3 @@",
        " twenty",
        "+twenty-one",
    ]
    .iter()
    .map(|line| (*line).to_owned())
    .collect()
}

#[test]
fn hunks_span_from_each_header_to_the_next() {
    let found = hunks(&patch());
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].header, 3);
    assert_eq!(found[0].end, 6);
    assert_eq!(found[0].label, "@@ -1,3 +1,4 @@");
    assert_eq!(found[1].header, 6);
    assert_eq!(found[1].end, 9);
    assert!(found[0].contains(3));
    assert!(found[0].contains(5));
    assert!(!found[0].contains(6));
}

#[test]
fn a_patch_without_hunks_indexes_nothing() {
    assert!(hunks(&[]).is_empty());
    assert!(hunks(&["diff --git a/x b/x".to_owned()]).is_empty());
}

#[test]
fn hunk_navigation_stops_at_both_ends_instead_of_wrapping() {
    let found = hunks(&patch());
    assert_eq!(next_hunk(&found, 0), Some(3));
    assert_eq!(next_hunk(&found, 3), Some(6));
    assert_eq!(next_hunk(&found, 6), None);
    assert_eq!(previous_hunk(&found, 8), Some(6));
    assert_eq!(previous_hunk(&found, 6), Some(3));
    assert_eq!(previous_hunk(&found, 3), None);
}

#[test]
fn origins_render_as_one_combined_label() {
    assert_eq!(
        origin_label(&[ChangeOrigin::Committed, ChangeOrigin::Unstaged]),
        "commit+unstaged"
    );
    assert_eq!(origin_label(&[ChangeOrigin::Untracked]), "untracked");
    assert_eq!(origin_label(&[]), "");
    assert_eq!(ChangeOrigin::Staged.to_string(), "staged");
}

#[test]
fn anchors_describe_themselves_in_one_based_terms() {
    assert_eq!(CommentAnchor::File.describe(), "file");
    assert_eq!(CommentAnchor::Hunk(0).describe(), "hunk 1");
    assert_eq!(CommentAnchor::Line(41).describe(), "line 42");
}

#[test]
fn comments_are_edited_in_place_at_the_same_anchor() {
    let path = Path::new("src/lib.rs");
    let mut comments = ReviewComments::default();
    assert!(comments.is_empty());

    assert!(comments.upsert(path, CommentAnchor::Line(4), "needs a test"));
    assert!(comments.upsert(path, CommentAnchor::Line(4), "needs two tests"));
    assert_eq!(comments.len(), 1);
    assert_eq!(
        comments.body(path, CommentAnchor::Line(4)),
        Some("needs two tests")
    );
}

#[test]
fn distinct_anchors_on_one_file_are_kept_apart() {
    let path = Path::new("src/lib.rs");
    let other = Path::new("src/main.rs");
    let mut comments = ReviewComments::default();
    comments.upsert(path, CommentAnchor::File, "overall fine");
    comments.upsert(path, CommentAnchor::Hunk(1), "second hunk");
    comments.upsert(path, CommentAnchor::Line(4), "this line");
    comments.upsert(other, CommentAnchor::File, "elsewhere");

    assert_eq!(comments.count_for(path), 3);
    assert_eq!(comments.count_for(other), 1);
    assert_eq!(comments.len(), 4);
    assert_eq!(comments.iter().count(), 4);
    assert_eq!(
        comments
            .for_path(path)
            .map(|comment| comment.anchor)
            .collect::<Vec<_>>(),
        vec![
            CommentAnchor::File,
            CommentAnchor::Hunk(1),
            CommentAnchor::Line(4)
        ]
    );
}

#[test]
fn clearing_the_body_removes_the_comment() {
    let path = Path::new("src/lib.rs");
    let mut comments = ReviewComments::default();
    comments.upsert(path, CommentAnchor::Line(4), "temporary");
    assert!(!comments.upsert(path, CommentAnchor::Line(4), "   "));
    assert_eq!(comments.body(path, CommentAnchor::Line(4)), None);
    assert!(comments.is_empty());
}

#[test]
fn an_empty_body_at_a_fresh_anchor_stores_nothing() {
    let mut comments = ReviewComments::default();
    assert!(!comments.upsert(Path::new("a.rs"), CommentAnchor::File, ""));
    assert!(comments.is_empty());
}
