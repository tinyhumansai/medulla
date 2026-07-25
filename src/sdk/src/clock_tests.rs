//! Tests for the clock module.

use super::*;

#[test]
fn now_millis_and_nanos_are_positive_and_ordered() {
    assert!(now_millis() > 0);
    assert!(now_nanos() > 0);
    // millis and nanos read the same clock; nanos is the finer unit.
    assert!(now_nanos() as i64 / 1_000_000 >= now_millis() - 1);
}
