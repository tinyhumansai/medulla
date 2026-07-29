//! Small display helpers shared by the views.

/// 24h clock `HH:MM:SS` for an epoch-ms timestamp (UTC — no tz database here).
pub fn clock(millis: i64) -> String {
    let secs = millis.div_euclid(1000).rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// Collapse internal whitespace, trim, and ellipsize to `width` chars.
pub fn clip(value: &str, width: usize) -> String {
    let single = collapse_ws(value);
    let len = single.chars().count();
    if len > width {
        let take = width.saturating_sub(1);
        let mut out: String = single.chars().take(take).collect();
        out.push('…');
        out
    } else {
        single
    }
}

/// Like [`clip`], but drops characters from the *front*.
///
/// For filesystem paths, where the tail identifies the thing and the head is
/// usually a home directory every candidate shares: clipping a path from the
/// right leaves several rows reading `/Users/someone/work/…`, which distinguishes
/// nothing.
pub fn clip_left(value: &str, width: usize) -> String {
    let single = collapse_ws(value);
    let len = single.chars().count();
    if len <= width {
        return single;
    }
    // One char is spent on the leading ellipsis, so keep `width - 1` from the
    // tail. A width of 0 or 1 leaves nothing meaningful; return just the marker
    // rather than panicking on the arithmetic.
    let keep = width.saturating_sub(1);
    let skip = len.saturating_sub(keep);
    let mut out = String::from("…");
    out.extend(single.chars().skip(skip));
    out
}

fn collapse_ws(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// How many characters of an address survive at each end.
const ADDRESS_EDGE: usize = 4;

/// Shorten a machine address for display: `abcd…1234`.
///
/// A tiny.place address is a Solana public key — 32 to 44 base58 characters of
/// pure entropy. Rendered whole it is the widest thing in the Agents rail, and
/// it sizes the whole sidebar, because the rail is measured against its widest
/// row. It also distinguishes nothing a human reads: the middle is noise, and
/// two peers are told apart by their ends.
///
/// Clipping from the right ([`clip`]) is the wrong shape here — several peers'
/// keys can share a prefix, so `AbCdEf…` rows would all look alike. Keeping both
/// ends is what keeps them distinguishable.
///
/// Unconditional: it shortens anything longer than the shortened form, so it is
/// for slots that are *known* to hold an opaque id. A slot that might hold a
/// name the operator chose wants [`short_if_address`] instead — this function
/// would happily turn `this-device` into `this…vice`.
pub fn short_address(value: &str) -> String {
    let trimmed = value.trim();
    let chars: Vec<char> = trimmed.chars().collect();
    // The ellipsis has to save at least one character to be worth printing.
    if chars.len() <= ADDRESS_EDGE * 2 + 1 {
        return trimmed.to_string();
    }
    let head: String = chars.iter().take(ADDRESS_EDGE).collect();
    let tail: String = chars.iter().skip(chars.len() - ADDRESS_EDGE).collect();
    format!("{head}…{tail}")
}

/// Whether `value` looks like a bare machine address rather than a name someone
/// chose.
///
/// Shortening is only ever right for the former: a label, a handle, or a
/// workspace path is meaningful text, and cutting its middle out destroys the
/// thing that made it readable. A base58 public key has no spaces, no
/// punctuation a human would type, and a length in the key range.
pub fn looks_like_address(value: &str) -> bool {
    let trimmed = value.trim();
    (32..=44).contains(&trimmed.chars().count())
        && trimmed.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Shorten `value` only if it looks like a bare address, otherwise return it
/// unchanged.
///
/// The safe default for a display slot that *might* hold an address and might
/// hold a name the operator gave the machine.
pub fn short_if_address(value: &str) -> String {
    if looks_like_address(value) {
        short_address(value)
    } else {
        value.trim().to_string()
    }
}

/// Compact token count, e.g. `980` · `1.2k` · `34k`.
pub fn fmt_tokens(n: i64) -> String {
    if n < 1_000 {
        return n.to_string();
    }
    // Millions get their own unit, or a 1M context window renders as `1000k` —
    // technically correct, and the reason the rail read `ctx 1.2k/1000k` where
    // it should say `1M`. A round million drops the `.0`, matching
    // [`crate::ui::meters::tokens`] so the rail and the meter agree.
    if n >= 1_000_000 {
        let m = n as f64 / 1_000_000.0;
        return if (m - m.round()).abs() < f64::EPSILON {
            format!("{}M", m.round() as i64)
        } else {
            format!("{m:.1}M")
        };
    }
    let k = n as f64 / 1_000.0;
    if k >= 10.0 {
        format!("{}k", k.round() as i64)
    } else {
        format!("{:.1}k", k)
    }
}

/// Word-wrap `text` to `width` columns, expanding tabs to two spaces and
/// preserving hard newlines. Long unbreakable runs are hard-cut at `width`.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for raw in text.split('\n') {
        let line = raw.replace('\t', "  ");
        let line = line.trim_end();
        if line.chars().count() <= width {
            out.push(line.to_string());
            continue;
        }
        let mut rest: Vec<char> = line.chars().collect();
        while rest.len() > width {
            // Last space at or before `width`.
            let cut = rest[..=width.min(rest.len() - 1)]
                .iter()
                .rposition(|&c| c == ' ')
                .filter(|&i| i > 0)
                .unwrap_or(width);
            let head: String = rest[..cut].iter().collect();
            out.push(head);
            let mut tail: Vec<char> = rest[cut..].to_vec();
            while tail.first() == Some(&' ') {
                tail.remove(0);
            }
            rest = tail;
        }
        if !rest.is_empty() {
            out.push(rest.into_iter().collect());
        }
    }
    out
}

/// Braille spinner frames shared by the app, login, and onboarding screens.
pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

#[cfg(test)]
#[path = "util_tests.rs"]
mod tests;
