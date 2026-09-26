//! Wall-clock helpers shared across the crate.
//!
//! These centralize the epoch-time reads that were previously copy-pasted as
//! private `now_ms`/`now_millis` helpers in several modules. Both return `0` on
//! the (practically impossible) case of a clock set before the Unix epoch, so
//! callers never have to handle the error.

use std::time::{SystemTime, UNIX_EPOCH};

/// The largest `|year|` [`epoch_millis_from_iso`] will attempt to convert.
///
/// Conservatively inside the roughly ±292,277,024-year range i64 milliseconds
/// since the epoch can represent — see the call site for why it has to be
/// checked before, not just during, the civil-date conversion.
const MAX_YEAR: u64 = 292_000_000;

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

/// Parse an ISO-8601 instant into milliseconds since the Unix epoch.
///
/// The inverse of [`iso_from_epoch_millis`], and hand-rolled for the same
/// reason: this and that formatter are the only date handling the SDK does, so
/// a date crate would be carried for two functions.
///
/// Accepts what a JSON API actually emits — `YYYY-MM-DDTHH:MM:SS` with optional
/// fractional seconds, and either a `Z` suffix or a `±HH:MM` offset. A space in
/// place of the `T` is accepted too, because some serializers emit one. Returns
/// `None` for anything it cannot read in full rather than guessing: a caller
/// deciding entitlement from a timestamp must be able to tell "this is old" from
/// "I could not read this".
pub fn epoch_millis_from_iso(text: &str) -> Option<i64> {
    let text = text.trim();
    let (date, rest) = text.split_once(['T', 't', ' '])?;

    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    // Bounded before it ever reaches days_from_civil, not just before the
    // checked arithmetic below: that helper's own `era * 146_097` overflows i64
    // for a year anywhere near its bare numeric range (a 19-digit year parses
    // fine as an i64), which would panic a debug build before the checked
    // arithmetic downstream got a chance to catch it. i64 milliseconds since the
    // epoch cover roughly ±292,277,024 years; MAX_YEAR stays comfortably inside
    // that so every step from here down — including days_from_civil's internal
    // multiplication — has room.
    //
    // Validated against the actual length of `month` (leap years included), not
    // just the 1..=31 range: `2026-02-31` is the wrong shape to reject on range
    // alone, and days_from_civil below normalizes an out-of-range day into the
    // next month rather than rejecting it — silently turning a bad payload into
    // a plausible-looking, wrong date.
    if date_parts.next().is_some()
        || year.unsigned_abs() > MAX_YEAR
        || !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
    {
        return None;
    }

    // Split the zone off before parsing the time. The offset sign is searched
    // from the end so it cannot be confused with anything earlier in the string.
    let (time, offset_minutes) = split_zone(rest)?;

    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let (second, milli) = match time_parts.next() {
        Some(sec) => parse_seconds(sec)?,
        None => (0, 0),
    };
    if time_parts.next().is_some() || !(0..24).contains(&hour) || !(0..60).contains(&minute) {
        return None;
    }
    // 60 is allowed: a leap second is a real value to receive, and clamping it
    // to the next minute is closer than refusing the whole timestamp.
    if !(0..=60).contains(&second) {
        return None;
    }

    // Checked from here down: `year` came off the wire as an unrestricted i64,
    // so a payload naming a year far outside anything real (`"1000000000-01-01"`)
    // must be refused rather than let `days * 86_400` or `seconds * 1_000`
    // overflow — a debug build would panic on startup, and a release build would
    // wrap into an arbitrary instant and hand entitlement an answer for the
    // wrong day instead of the `None` a caller can fail closed on.
    let days = days_from_civil(year, month, day);
    let seconds = days
        .checked_mul(86_400)?
        .checked_add(hour * 3_600)?
        .checked_add(minute * 60)?
        .checked_add(second)?
        .checked_sub(offset_minutes * 60)?;
    seconds.checked_mul(1_000)?.checked_add(milli)
}

/// Split a trailing `Z` or `±HH:MM`/`±HHMM` zone off a time, returning the time
/// and the zone's offset in minutes east of UTC.
///
/// A missing zone is read as UTC. That is the lenient half of this parser and it
/// is deliberate: every timestamp the backend sends is UTC, and refusing a naive
/// one would fail closed on a serializer change rather than on real ambiguity.
fn split_zone(rest: &str) -> Option<(&str, i64)> {
    if let Some(time) = rest.strip_suffix(['Z', 'z']) {
        return Some((time, 0));
    }
    // Skip index 0: a leading sign would mean there is no time at all.
    if let Some(idx) = rest.rfind(['+', '-']).filter(|&i| i > 0) {
        let (time, zone) = rest.split_at(idx);
        let sign = if zone.starts_with('-') { -1 } else { 1 };
        let zone = &zone[1..];
        let (hours, minutes) = match zone.split_once(':') {
            Some((h, m)) => (h, m),
            // `±HHMM`, the compact spelling.
            None if zone.len() == 4 => zone.split_at(2),
            None if zone.len() == 2 => (zone, "0"),
            None => return None,
        };
        let hours: i64 = hours.parse().ok()?;
        let minutes: i64 = minutes.parse().ok()?;
        if !(0..=23).contains(&hours) || !(0..=59).contains(&minutes) {
            return None;
        }
        return Some((time, sign * (hours * 60 + minutes)));
    }
    Some((rest, 0))
}

/// Parse a seconds field that may carry a fraction, into `(seconds, millis)`.
///
/// Fractions finer than a millisecond are truncated rather than rounded, and
/// shorter ones are padded — `.5` is 500ms, not 5ms.
fn parse_seconds(field: &str) -> Option<(i64, i64)> {
    let (whole, fraction) = match field.split_once('.') {
        Some((w, f)) => (w, f),
        None => (field, ""),
    };
    let seconds: i64 = whole.parse().ok()?;
    if fraction.is_empty() {
        return Some((seconds, 0));
    }
    if !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut millis = 0i64;
    for i in 0..3 {
        millis = millis * 10 + i64::from(fraction.as_bytes().get(i).map_or(0, |b| b - b'0'));
    }
    Some((seconds, millis))
}

/// Whether `year` is a leap year in the proleptic Gregorian calendar.
fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// The number of days in `month` of `year`, or `0` for an out-of-range month.
///
/// Used only to validate an incoming day-of-month before it reaches
/// [`days_from_civil`], which has no invalid-date case of its own: it treats
/// day 31 of a 30-day month as day 1 of the next one instead of rejecting it.
fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// `(year, month, day)` → days since 1970-01-01.
///
/// Howard Hinnant's `days_from_civil`, the exact inverse of [`civil_from_days`].
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
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
