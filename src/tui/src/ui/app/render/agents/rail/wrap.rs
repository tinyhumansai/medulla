//! Line- and path-wrapping for the Agents rail.
//!
//! A rail row is a fixed number of columns wide; this module is the only place
//! that decides how a row (or the working directory inside a harness row) is
//! cut across lines when it does not fit. Every measurement here is in
//! terminal cells rather than `char`s or bytes — a CJK glyph or emoji occupies
//! two columns, and treating it as one would accept a line that actually
//! overruns the pane.

use ratatui::style::Style;
use ratatui::text::{Line as TLine, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Re-flow one rail row across as many lines as `width` needs, keeping styles.
///
/// Works on characters rather than on the row's text, because a row is several
/// spans and its colours carry meaning — a task row's status word is coloured by
/// status, and re-wrapping through a plain `String` would hand the whole row one
/// style. Continuation lines are indented so a wrapped row still reads as one.
pub(super) fn wrap_line(line: &TLine<'static>, width: usize, indent: usize) -> Vec<TLine<'static>> {
    let width = width.max(8);
    let cells: Vec<(char, Style)> = line
        .spans
        .iter()
        .flat_map(|span| span.content.chars().map(move |c| (c, span.style)))
        .collect();
    if cell_width(&cells) <= width {
        return vec![line.clone()];
    }
    let indent = indent.min(width.saturating_sub(4));
    let mut out = Vec::new();
    let mut start = 0;
    let mut first = true;
    while start < cells.len() {
        let pad = if first { 0 } else { indent };
        let room = width - pad;
        let end = width_limited_end(&cells, start, room);
        // Break on the last space inside the window so a wrap falls between
        // words. A window with no space in it (a long address, a path) is cut
        // at the edge — there is nothing better to break on.
        let cut = if end == cells.len() {
            end
        } else {
            cells[start..end]
                .iter()
                .rposition(|(c, _)| *c == ' ')
                .map(|offset| start + offset)
                .filter(|&at| at > start)
                .unwrap_or(end)
        };
        out.push(styled_line(&cells[start..cut], pad));
        start = cut;
        // The space a break landed on is consumed by the break itself; leading
        // blanks on the next line would double the indent.
        while matches!(cells.get(start), Some((' ', _))) {
            start += 1;
        }
        first = false;
    }
    out
}

/// Sum of terminal-cell widths of `cells`, per [`UnicodeWidthChar`].
fn cell_width(cells: &[(char, Style)]) -> usize {
    cells.iter().map(|(c, _)| c.width().unwrap_or(0)).sum()
}

/// The largest `end >= start` such that `cells[start..end]` fits in `room`
/// terminal columns. Always advances by at least one cell so an over-wide
/// single character does not stall the loop.
fn width_limited_end(cells: &[(char, Style)], start: usize, room: usize) -> usize {
    let mut used = 0;
    let mut end = start;
    for (c, _) in &cells[start..] {
        let w = c.width().unwrap_or(0);
        if end > start && used + w > room {
            break;
        }
        used += w;
        end += 1;
    }
    end
}

/// Rebuild a line from styled characters, merging neighbours that share a style.
fn styled_line(cells: &[(char, Style)], pad: usize) -> TLine<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    if pad > 0 {
        spans.push(Span::raw(" ".repeat(pad)));
    }
    for (c, style) in cells {
        match spans.last_mut() {
            // Neighbouring characters of one style are one span; the padding
            // span merges the same way when the first character is unstyled,
            // which is exactly what it should look like.
            Some(last) if last.style == *style => last.content.to_mut().push(*c),
            _ => spans.push(Span::styled(c.to_string(), *style)),
        }
    }
    TLine::from(spans)
}

/// The operator's home directory, however this platform names it.
///
/// [`dirs::home_dir`] rather than `$HOME`: Windows sets `USERPROFILE` and no
/// `HOME` at all, so reading the variable directly meant the collapse below
/// silently never fired there — and a test that read it panicked outright.
pub(super) fn home_dir() -> Option<String> {
    let home = dirs::home_dir()?;
    let home = home.to_string_lossy();
    let trimmed = home.trim_end_matches(['/', '\\']);
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// A working directory with `home` collapsed to `~`.
///
/// Every harness on a laptop starts under the same home directory, so spelling
/// it out on every row spends columns on the one part of the path that
/// distinguishes nothing.
///
/// Takes the home directory rather than reading the environment so the rule is
/// a pure function of its inputs: this is presentation logic, and a renderer
/// that consults the process environment cannot be tested on a machine that
/// disagrees with the fixture.
pub(super) fn short_home(path: &str, home: Option<&str>) -> String {
    // Both separators, because the path being shortened comes from the harness
    // and the home directory from the platform — on Windows those disagree.
    let trimmed = path.trim_end_matches(['/', '\\']);
    let trimmed = if trimmed.is_empty() { "/" } else { trimmed };
    let Some(home) = home.map(|h| h.trim_end_matches(['/', '\\'])) else {
        return trimmed.to_string();
    };
    if home.is_empty() {
        return trimmed.to_string();
    }
    match trimmed.strip_prefix(home) {
        Some("") => "~".to_string(),
        Some(rest) if rest.starts_with(['/', '\\']) => format!("~{rest}"),
        _ => trimmed.to_string(),
    }
}

/// Break a path across at most `max_lines` lines of `width` columns.
///
/// A path too long for that many lines loses leading segments, not trailing
/// ones: the tail names the checkout, while the head is the part every sibling
/// harness on the machine already shares. What was dropped is marked with a
/// leading `…` so the row never reads as an absolute path it is not.
pub(super) fn wrap_path(path: &str, width: usize, max_lines: usize) -> Vec<String> {
    let width = width.max(4);
    let max_lines = max_lines.max(1);
    let mut text = path.to_string();
    loop {
        let lines = flow_path(&text, width);
        if lines.len() <= max_lines {
            return lines;
        }
        // Drop the leading segment and try again. Each pass removes one, so
        // this reaches either a fit or a path with no separators left.
        let head = text.trim_start_matches('…');
        match head.split_once('/') {
            Some((_, tail)) if !tail.is_empty() => text = format!("…{tail}"),
            // A single unbreakable segment: it has no separator left to drop
            // a head at, so cut characters directly. The tail is what
            // distinguishes this checkout from a sibling sharing the same
            // prefix, so keep the last characters that fit and mark the
            // dropped head with a leading `…` rather than keeping the head
            // and losing the tail.
            _ => {
                let budget = width.saturating_mul(max_lines).saturating_sub(1).max(1);
                // Walk from the end so the kept tail is bounded by display
                // width, not `char` count — a wide character counted as one
                // column would let the kept slice overrun the line budget.
                let chars: Vec<char> = head.chars().collect();
                let mut used = 0;
                let mut cut = chars.len();
                for (i, c) in chars.iter().enumerate().rev() {
                    let w = c.width().unwrap_or(0);
                    if used + w > budget {
                        break;
                    }
                    used += w;
                    cut = i;
                }
                let tail: String = chars[cut..].iter().collect();
                let mut lines = flow_path(&format!("…{tail}"), width);
                lines.truncate(max_lines);
                return lines;
            }
        }
    }
}

/// Lay a path out on `/` boundaries, one line per run of whole segments.
///
/// A path cut mid-segment is harder to read than one that wraps a segment
/// early, so a segment moves to the next line whole. One wider than the pane
/// has no separator to break on and is hard-cut rather than allowed to overflow.
pub(super) fn flow_path(path: &str, width: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    for segment in path.split_inclusive('/') {
        if !current.is_empty() && current.width() + segment.width() > width {
            out.push(std::mem::take(&mut current));
        }
        if segment.width() > width {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            out.extend(chunk_by_width(segment, width));
            continue;
        }
        current.push_str(segment);
    }
    if !current.is_empty() {
        out.push(current);
    }
    if out.is_empty() {
        out.push("/".to_string());
    }
    out
}

/// Split `s` into chunks of at most `width` terminal columns each, in order.
///
/// Used where a path segment has no `/` to break on: cutting by `char` count
/// instead of display width would under- or over-fill a chunk containing wide
/// (e.g. CJK) characters.
fn chunk_by_width(s: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    let mut chunk = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used > 0 && used + w > width {
            out.push(std::mem::take(&mut chunk));
            used = 0;
        }
        chunk.push(c);
        used += w;
    }
    if !chunk.is_empty() {
        out.push(chunk);
    }
    out
}
