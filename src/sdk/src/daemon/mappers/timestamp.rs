//! Timestamp parsing: fold a transcript record's ISO-8601 string into epoch
//! milliseconds, with a dependency-free RFC3339 parser and a receive-time
//! fallback so a live session's derived status clock never reads as stale.

use serde_json::Value;

/// Epoch ms for an ISO-8601 string, falling back to receive time. Missing or
/// unparseable timestamps default to *now* (not the Unix epoch), mirroring the
/// TS `parseTimestamp` so the derived status clock never treats a live session
/// as stale.
pub(super) fn parse_timestamp_ms(value: Option<&Value>) -> i64 {
    match value.and_then(Value::as_str) {
        Some(text) => parse_iso_to_ms(text).unwrap_or_else(now_ms),
        None => now_ms(),
    }
}

/// Current wall-clock time as epoch milliseconds (0 if the clock predates epoch).
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Parse an RFC3339 UTC-ish instant to epoch ms, or `None` if unreadable.
/// Public wrapper over the internal parser so the wrapper's envelope builder can
/// reuse one RFC3339 implementation.
pub fn parse_iso_ms(text: &str) -> Option<i64> {
    parse_iso_to_ms(text)
}

/// Minimal RFC3339 parser (`YYYY-MM-DDTHH:MM:SS(.fff)?(Z|±HH:MM)?`) → epoch ms.
/// Dependency-free; returns `None` for anything it cannot read.
pub(super) fn parse_iso_to_ms(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let year: i64 = text.get(0..4)?.parse().ok()?;
    if bytes[4] != b'-' {
        return None;
    }
    let month: i64 = text.get(5..7)?.parse().ok()?;
    if bytes[7] != b'-' {
        return None;
    }
    let day: i64 = text.get(8..10)?.parse().ok()?;
    if bytes[10] != b'T' && bytes[10] != b' ' {
        return None;
    }
    let hour: i64 = text.get(11..13)?.parse().ok()?;
    if bytes[13] != b':' {
        return None;
    }
    let minute: i64 = text.get(14..16)?.parse().ok()?;
    if bytes[16] != b':' {
        return None;
    }
    let second: i64 = text.get(17..19)?.parse().ok()?;

    let mut index = 19;
    let mut millis: i64 = 0;
    if index < bytes.len() && bytes[index] == b'.' {
        index += 1;
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        let frac = text.get(start..index)?;
        // Take up to 3 digits for milliseconds; pad if shorter.
        let mut frac_ms = String::new();
        for (position, digit) in frac.chars().enumerate() {
            if position == 3 {
                break;
            }
            frac_ms.push(digit);
        }
        while frac_ms.len() < 3 {
            frac_ms.push('0');
        }
        millis = frac_ms.parse().ok()?;
    }

    // Timezone offset.
    let mut offset_minutes: i64 = 0;
    if index < bytes.len() {
        match bytes[index] {
            b'Z' | b'z' => {}
            b'+' | b'-' => {
                let sign = if bytes[index] == b'-' { -1 } else { 1 };
                let off_h: i64 = text.get(index + 1..index + 3)?.parse().ok()?;
                let off_m: i64 = text.get(index + 4..index + 6)?.parse().ok()?;
                offset_minutes = sign * (off_h * 60 + off_m);
            }
            _ => {}
        }
    }

    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset_minutes * 60;
    Some(seconds * 1000 + millis)
}

/// Days since the Unix epoch for a proleptic-Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
