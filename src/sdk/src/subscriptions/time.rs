//! Just enough RFC 3339 to turn a provider's reset timestamp into epoch seconds.
//!
//! Hand-rolled for the same reason [`crate::clock::iso_from_epoch_millis`] is:
//! this workspace carries no date crate, and the shapes that actually arrive
//! here are two — `2026-08-27T16:49:59.653759+00:00` from Claude, and a bare
//! integer from Codex, which never reaches this module. Anything else parses to
//! `None` and the reading simply loses its reset time.

/// Parse an RFC 3339 timestamp into seconds since the Unix epoch.
///
/// Accepts `Z`, `+HH:MM`/`-HH:MM`, and a missing offset (read as UTC), with or
/// without fractional seconds. Returns `None` for anything it cannot read
/// exactly, rather than a guess: a wrong reset time is worse than none.
pub fn parse_rfc3339(value: &str) -> Option<i64> {
    let value = value.trim();
    let (date, rest) = value.split_once(['T', 't', ' '])?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    // Split the offset off the time before parsing it: the sign characters
    // cannot appear anywhere else in the time, so the first one after position
    // zero starts the offset.
    let (time, offset_seconds) = split_offset(rest)?;
    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    // Seconds may carry a fraction, which is below this function's resolution.
    let seconds_field = time_parts.next().unwrap_or("0");
    let second: i64 = seconds_field
        .split_once('.')
        .map(|(whole, _)| whole)
        .unwrap_or(seconds_field)
        .parse()
        .ok()?;
    if time_parts.next().is_some() || !(0..24).contains(&hour) || !(0..60).contains(&minute) {
        return None;
    }

    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second - offset_seconds)
}

/// Split a time-of-day from its trailing UTC offset, in seconds east of UTC.
fn split_offset(rest: &str) -> Option<(&str, i64)> {
    if let Some(time) = rest.strip_suffix(['Z', 'z']) {
        return Some((time, 0));
    }
    let sign_at = rest
        .char_indices()
        .skip(1)
        .find(|(_, c)| *c == '+' || *c == '-')
        .map(|(index, _)| index);
    let Some(sign_at) = sign_at else {
        // No offset at all: RFC 3339 requires one, but providers omit it and
        // mean UTC, which is also what a naive local reading would get wrong.
        return Some((rest, 0));
    };
    let (time, offset) = rest.split_at(sign_at);
    let negative = offset.starts_with('-');
    let offset = &offset[1..];
    let (hours, minutes) = match offset.split_once(':') {
        Some((hours, minutes)) => (hours, minutes),
        None if offset.len() == 4 => offset.split_at(2),
        None => (offset, "0"),
    };
    let hours: i64 = hours.parse().ok()?;
    let minutes: i64 = minutes.parse().ok()?;
    let total = hours * 3_600 + minutes * 60;
    Some((time, if negative { -total } else { total }))
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`, the same algorithm the clock module's inverse uses).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}
