//! Unit tests for [`super::CellText`].
use super::*;

#[test]
fn short_text_stays_inline() {
    for text in ["", " ", "a", "é", "▀", "😀"] {
        let cell = CellText::from(text);
        assert!(cell.is_inline(), "{text:?} should not allocate");
        assert_eq!(cell.as_str(), text);
    }
}

#[test]
fn a_long_grapheme_cluster_falls_back_to_the_heap() {
    // A ZWJ family emoji is 25 bytes — past the inline capacity, and the
    // reason the fallback exists rather than truncating.
    let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}";
    assert!(family.len() > INLINE_CAP);
    let cell = CellText::from(family);
    assert!(!cell.is_inline());
    assert_eq!(cell.as_str(), family);
}

#[test]
fn the_boundary_length_is_still_inline() {
    let text = "x".repeat(INLINE_CAP);
    assert!(CellText::from(text.as_str()).is_inline());
    assert_eq!(CellText::from(text.as_str()).as_str(), text);

    let over = "x".repeat(INLINE_CAP + 1);
    assert!(!CellText::from(over.as_str()).is_inline());
}

#[test]
fn default_is_empty() {
    assert!(CellText::default().is_empty());
    assert_eq!(CellText::default().as_str(), "");
}

#[test]
fn it_is_no_wider_than_the_string_it_replaced() {
    // The point of the inline representation is to remove an allocation, not
    // to trade it for a fatter snapshot.
    assert!(std::mem::size_of::<CellText>() <= std::mem::size_of::<String>());
}

#[test]
fn equality_matches_the_underlying_text() {
    assert_eq!(CellText::from("ab"), CellText::from("ab"));
    assert_ne!(CellText::from("ab"), CellText::from("ac"));
    // Bound first: comparing a temporary trips `clippy::cmp_owned`, which
    // does not know that a short `CellText` never allocates.
    let cell = CellText::from("ab");
    assert!(cell == *"ab");
    assert!(cell == "ab");
}
