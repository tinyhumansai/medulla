//! Small display helpers shared by the views.

/// 24h clock `HH:MM:SS` for an epoch-ms timestamp (UTC — no tz database here).
pub fn clock(millis: i64) -> String {
    let secs = millis.div_euclid(1000).rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
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

fn collapse_ws(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Compact token count, e.g. `980` · `1.2k` · `34k`.
pub fn fmt_tokens(n: i64) -> String {
    if n < 1_000 {
        return n.to_string();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_formats_utc() {
        assert_eq!(clock(0), "00:00:00");
        assert_eq!(clock(3_661_000), "01:01:01");
    }

    #[test]
    fn clip_collapses_and_ellipsizes() {
        assert_eq!(clip("a   b\tc", 10), "a b c");
        assert_eq!(clip("abcdefgh", 4), "abc…");
    }

    #[test]
    fn fmt_tokens_scales() {
        assert_eq!(fmt_tokens(980), "980");
        assert_eq!(fmt_tokens(1_200), "1.2k");
        assert_eq!(fmt_tokens(34_000), "34k");
    }

    #[test]
    fn wrap_breaks_on_spaces() {
        let w = wrap("the quick brown fox", 9);
        assert_eq!(w, vec!["the quick", "brown fox"]);
        // Hard-cut a long unbreakable run.
        let hard = wrap("abcdefghij", 4);
        assert_eq!(hard, vec!["abcd", "efgh", "ij"]);
    }
}
