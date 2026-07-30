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
    // A 1M context window must not render as `1000k`.
    assert_eq!(fmt_tokens(1_000_000), "1M");
    assert_eq!(fmt_tokens(999_999), "1000k");
    assert_eq!(fmt_tokens(1_500_000), "1.5M");
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

#[test]
fn clip_middle_keeps_a_paths_root_and_project_name() {
    assert_eq!(clip_middle("/work/repo", 20), "/work/repo");
    assert_eq!(
        clip_middle("/Users/me/work/tinyhumansai/medulla-public", 20),
        "/Users/me/…la-public"
    );
    assert_eq!(clip_middle("/work/repo", 1), "…");
    assert_eq!(clip_middle("/work/repo", 0), "…");
}

#[test]
fn an_address_keeps_both_ends_so_two_peers_stay_distinguishable() {
    // A real 44-character base58 Solana public key.
    let key = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
    assert_eq!(short_address(key), "7xKX…gAsU");
    // Two keys sharing a long prefix still read differently, which clipping
    // from the right alone would not achieve.
    let sibling = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosZZZZ";
    assert_ne!(short_address(key), short_address(sibling));
}

#[test]
fn something_already_short_is_left_alone_rather_than_mangled() {
    assert_eq!(short_address("abc"), "abc");
    // Exactly at the threshold: an ellipsis would not save a character.
    assert_eq!(short_address("123456789"), "123456789");
    assert_eq!(short_address("1234567890"), "1234…7890");
    // `short_address` is unconditional, so a chosen name IS cut by it — which
    // is exactly why display slots that might hold one use `short_if_address`.
    assert_eq!(short_address("this-device"), "this…vice");
    assert_eq!(short_if_address("this-device"), "this-device");
}

#[test]
fn only_things_that_look_like_addresses_are_shortened() {
    let key = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
    assert!(looks_like_address(key));
    assert_eq!(short_if_address(key), "7xKX…gAsU");

    // A name someone chose is meaningful text: cutting its middle out destroys
    // the thing that made it readable, however long it is.
    let named = "steven's macbook pro in the office";
    assert!(!looks_like_address(named));
    assert_eq!(short_if_address(named), named);
    // Punctuation rules out a base58 key, so paths and handles pass through.
    assert!(!looks_like_address("/Users/me/work/some-long-repo-name"));
    assert!(!looks_like_address("@a-fairly-long-handle-here"));
    // Too short to be a key, even though it is alphanumeric.
    assert!(!looks_like_address("laptop2"));
    assert_eq!(short_if_address("laptop2"), "laptop2");
}

#[test]
fn shortening_trims_surrounding_whitespace() {
    assert_eq!(short_if_address("  this-device  "), "this-device");
    assert_eq!(
        short_address("  7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU  "),
        "7xKX…gAsU"
    );
}
