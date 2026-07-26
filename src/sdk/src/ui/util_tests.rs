//! Tests for the util module.

use super::*;

#[test]
fn clock_formats_utc() {
    assert_eq!(clock(0), "00:00:00");
    assert_eq!(clock(3_661_000), "01:01:01");
}

#[test]
fn clip_collapses_and_ellipsizes() {
    assert_eq!(clip("a   b\tc", 10), "a b c");
    assert_eq!(clip("abcdefgh", 4), "abc…");
}

#[test]
fn fmt_tokens_scales() {
    assert_eq!(fmt_tokens(980), "980");
    assert_eq!(fmt_tokens(1_200), "1.2k");
    assert_eq!(fmt_tokens(34_000), "34k");
}

#[test]
fn wrap_breaks_on_spaces() {
    let w = wrap("the quick brown fox", 9);
    assert_eq!(w, vec!["the quick", "brown fox"]);
    // Hard-cut a long unbreakable run.
    let hard = wrap("abcdefghij", 4);
    assert_eq!(hard, vec!["abcd", "efgh", "ij"]);
}

#[test]
fn clip_left_keeps_the_tail_that_identifies_a_path() {
    // Short enough to survive untouched.
    assert_eq!(clip_left("/work/repo", 20), "/work/repo");
    // The distinguishing tail is kept; the shared prefix is what goes.
    assert_eq!(clip_left("/Users/me/work/repo", 10), "…work/repo");
    // Degenerate widths return the marker rather than panicking.
    assert_eq!(clip_left("/Users/me/work/repo", 1), "…");
    assert_eq!(clip_left("/Users/me/work/repo", 0), "…");
}
