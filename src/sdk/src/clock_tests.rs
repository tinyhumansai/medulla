//! Tests for the clock module.

use super::*;

#[test]
fn now_millis_and_nanos_are_positive_and_ordered() {
    assert!(now_millis() > 0);
    assert!(now_nanos() > 0);
    // millis and nanos read the same clock; nanos is the finer unit.
    assert!(now_nanos() as i64 / 1_000_000 >= now_millis() - 1);
}

#[test]
fn iso_now_has_the_fixed_wire_shape() {
    let stamp = iso_now();
    assert_eq!(stamp.len(), 24, "YYYY-MM-DDTHH:MM:SS.mmmZ — got {stamp}");
    assert!(stamp.ends_with('Z'), "{stamp}");
    assert_eq!(&stamp[4..5], "-");
    assert_eq!(&stamp[10..11], "T");
    assert_eq!(&stamp[19..20], ".");
}

#[test]
fn iso_rendering_matches_known_instants() {
    assert_eq!(iso_from_epoch_millis(0), "1970-01-01T00:00:00.000Z");
    // A leap day, to exercise the era-shifted civil-date conversion.
    assert_eq!(
        iso_from_epoch_millis(1_709_164_800_123),
        "2024-02-29T00:00:00.123Z"
    );
    assert_eq!(
        iso_from_epoch_millis(1_767_225_599_999),
        "2025-12-31T23:59:59.999Z"
    );
}
