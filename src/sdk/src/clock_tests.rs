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

#[test]
fn iso_parsing_matches_known_instants() {
    assert_eq!(epoch_millis_from_iso("1970-01-01T00:00:00.000Z"), Some(0));
    assert_eq!(
        epoch_millis_from_iso("2024-02-29T00:00:00.123Z"),
        Some(1_709_164_800_123)
    );
    assert_eq!(
        epoch_millis_from_iso("2025-12-31T23:59:59.999Z"),
        Some(1_767_225_599_999)
    );
}

/// The two halves must agree for every instant, which is the property that
/// keeps a hand-rolled pair honest.
#[test]
fn iso_parsing_round_trips_the_rendering() {
    for millis in [
        0,
        1_709_164_800_123,
        1_787_832_000_000,
        -86_400_000,
        253_402_300_799_000,
    ] {
        let rendered = iso_from_epoch_millis(millis);
        assert_eq!(
            epoch_millis_from_iso(&rendered),
            Some(millis),
            "round trip failed for {rendered}"
        );
    }
}

#[test]
fn iso_parsing_accepts_the_shapes_a_json_api_emits() {
    let expected = Some(1_787_832_000_000);
    // No fraction, lowercase `z`, a space separator, and a naive instant.
    assert_eq!(epoch_millis_from_iso("2026-08-27T12:00:00Z"), expected);
    assert_eq!(epoch_millis_from_iso("2026-08-27t12:00:00z"), expected);
    assert_eq!(epoch_millis_from_iso("2026-08-27 12:00:00Z"), expected);
    assert_eq!(epoch_millis_from_iso("2026-08-27T12:00:00"), expected);
    assert_eq!(epoch_millis_from_iso("  2026-08-27T12:00:00Z  "), expected);
    // Sub-millisecond precision truncates rather than rounding.
    assert_eq!(
        epoch_millis_from_iso("2026-08-27T12:00:00.123456Z"),
        Some(1_787_832_000_123)
    );
    // A short fraction pads: `.5` is half a second.
    assert_eq!(
        epoch_millis_from_iso("2026-08-27T12:00:00.5Z"),
        Some(1_787_832_000_500)
    );
}

#[test]
fn iso_parsing_applies_a_numeric_zone_offset() {
    let utc = Some(1_787_832_000_000);
    assert_eq!(epoch_millis_from_iso("2026-08-27T14:00:00+02:00"), utc);
    assert_eq!(epoch_millis_from_iso("2026-08-27T10:00:00-02:00"), utc);
    // The compact spellings.
    assert_eq!(epoch_millis_from_iso("2026-08-27T14:00:00+0200"), utc);
    assert_eq!(epoch_millis_from_iso("2026-08-27T14:00:00+02"), utc);
}

/// Refusing is the point: a caller deciding entitlement from a timestamp has to
/// be able to tell "this is old" from "I could not read this".
#[test]
fn iso_parsing_refuses_what_it_cannot_read_in_full() {
    for bad in [
        "",
        "not a date",
        "2026-08-27",
        "2026-08-27T12",
        "2026-13-01T00:00:00Z",
        "2026-08-32T00:00:00Z",
        // Impossible calendar dates: real months, day out of range for that
        // month. `days_from_civil` has no invalid-date case of its own — it
        // would silently normalize these into the following month instead of
        // refusing them, which is exactly the failure this list guards against.
        "2026-02-31T00:00:00Z",
        "2026-02-30T00:00:00Z",
        "2025-02-29T00:00:00Z", // 2025 is not a leap year
        "2026-04-31T00:00:00Z",
        "2026-00-01T00:00:00Z",
        "2026-08-00T00:00:00Z",
        "2026-08-27T24:00:00Z",
        "2026-08-27T12:60:00Z",
        "2026-08-27T12:00:61Z",
        "2026-08-27T12:00:00.12x3Z",
        "2026-08-27T12:00:00+99:00",
        "2026-08-27-01T12:00:00Z",
        // A year so large that days_from_civil's result overflows i64 once
        // multiplied out to milliseconds — must be refused, not panic (debug)
        // or wrap into an arbitrary instant (release).
        "1000000000-01-01T00:00:00Z",
        "-1000000000-01-01T00:00:00Z",
        // A year so large it overflows inside days_from_civil itself (its own
        // `era * 146_097`), before the checked arithmetic downstream ever runs.
        // A bare i64::MAX still parses as `year`, so this has to be bounded up
        // front rather than trusted to the later checked_mul chain.
        "9223372036854775807-01-01T00:00:00Z",
    ] {
        assert_eq!(
            epoch_millis_from_iso(bad),
            None,
            "should have refused {bad:?}"
        );
    }
}
