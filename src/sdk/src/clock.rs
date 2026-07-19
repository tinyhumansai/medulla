//! Wall-clock helpers shared across the crate.
//!
//! These centralize the epoch-time reads that were previously copy-pasted as
//! private `now_ms`/`now_millis` helpers in several modules. Both return `0` on
//! the (practically impossible) case of a clock set before the Unix epoch, so
//! callers never have to handle the error.

use std::time::{SystemTime, UNIX_EPOCH};

/// Current wall-clock time in milliseconds since the Unix epoch (0 on error).
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Current wall-clock time in nanoseconds since the Unix epoch (0 on error).
pub fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_millis_and_nanos_are_positive_and_ordered() {
        assert!(now_millis() > 0);
        assert!(now_nanos() > 0);
        // millis and nanos read the same clock; nanos is the finer unit.
        assert!(now_nanos() as i64 / 1_000_000 >= now_millis() - 1);
    }
}
