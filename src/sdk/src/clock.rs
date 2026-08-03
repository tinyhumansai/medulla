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

/// The current time as an ISO-8601 string, for log files and wire timestamps.
///
/// Millisecond precision with a `Z` suffix — the shape every timestamp this
/// workspace emits has always had, and one that already appears in persisted
/// session history and on the harness envelope wire, so the format is fixed.
pub fn iso_now() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    iso_from_epoch_millis(now.as_millis() as i64)
}

/// Render `millis` since the Unix epoch as `YYYY-MM-DDTHH:MM:SS.mmmZ`.
///
/// Hand-rolled rather than pulled from a date crate: this is the only date
/// formatting the SDK does, and the civil-date conversion below is the standard
/// days-from-epoch algorithm, exact for every year this code can see.
fn iso_from_epoch_millis(millis: i64) -> String {
    let (seconds, sub_milli) = (millis.div_euclid(1_000), millis.rem_euclid(1_000));
    let days = seconds.div_euclid(86_400);
    let time_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (
        time_of_day / 3_600,
        (time_of_day % 3_600) / 60,
        time_of_day % 60,
    );
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{sub_milli:03}Z")
}

/// Days since 1970-01-01 → `(year, month, day)`.
///
/// Howard Hinnant's `civil_from_days`, which shifts the era to start in March so
/// the leap day lands at the end of a year and needs no special case.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32;
    (year + i64::from(month <= 2), month, day)
}

#[cfg(test)]
#[path = "clock_tests.rs"]
mod tests;
