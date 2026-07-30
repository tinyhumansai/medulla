//! Tests for the Agents rail's line layout: the width cap, how a row that does
//! not fit is re-flowed, and what a working directory keeps when it cannot.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span};
use unicode_width::UnicodeWidthStr;

use super::wrap::{flow_path, short_home, wrap_line, wrap_path};

#[test]
fn a_row_that_fits_is_left_exactly_as_it_was() {
    let line = TLine::from(Span::raw("● orchestrator · 1"));
    let out = wrap_line(&line, 36, 5);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].width(), line.width());
}

#[test]
fn an_overlong_row_wraps_and_its_continuation_is_indented() {
    let line = TLine::from(Span::raw("● [CODEX] dev-1 · 3 · ctx 6.4k · 1/1 sess · 2/4"));
    let out = wrap_line(&line, 24, 5);

    assert!(out.len() > 1, "the row does not fit on one line");
    for wrapped in &out {
        assert!(
            wrapped.width() <= 24,
            "no line may overrun the pane: {wrapped:?}"
        );
    }
    let second: String = out[1].spans.iter().map(|s| s.content.to_string()).collect();
    assert!(
        second.starts_with("     "),
        "a continuation line is indented so the row still reads as one: {second:?}"
    );
}

#[test]
fn wrapping_keeps_each_span_style() {
    // A task row colours its status word by status. Re-flowing through a plain
    // string would hand the whole row one style and lose that entirely.
    let coloured = Style::default().add_modifier(Modifier::BOLD);
    let line = TLine::from(vec![
        Span::raw("   task-1 aaaa bbbb cccc "),
        Span::styled("running", coloured),
        Span::raw(" · 9 turns"),
    ]);
    let out = wrap_line(&line, 20, 5);

    let kept: usize = out
        .iter()
        .flat_map(|l| l.spans.iter())
        .filter(|span| span.style == coloured)
        .map(|span| span.content.chars().count())
        .sum();
    assert_eq!(kept, "running".len(), "the styled run survives the wrap");
}

#[test]
fn wide_characters_wrap_by_display_column_not_char_count() {
    // Each CJK character below occupies two terminal columns. A char-count
    // wrap would accept twice as many as actually fit and clip the row
    // instead of wrapping it.
    let line = TLine::from(Span::raw("任务一二三四五六七八九十"));
    let out = wrap_line(&line, 10, 0);

    assert!(out.len() > 1, "a row this wide must wrap: {out:?}");
    for wrapped in &out {
        assert!(
            wrapped.width() <= 10,
            "no line may overrun the pane: {wrapped:?}"
        );
    }
}

#[test]
fn a_path_breaks_on_separators() {
    let out = flow_path("~/work/tinyhumans/medulla", 12);
    for line in &out {
        assert!(line.chars().count() <= 12, "{line:?} overruns");
    }
    assert_eq!(out.concat(), "~/work/tinyhumans/medulla");
}

#[test]
fn a_path_too_long_to_fit_keeps_its_tail() {
    // The tail names the checkout; the head is what every sibling harness on
    // the machine shares, so dropping it loses nothing and dropping the tail
    // loses the only fact the row was drawn for.
    let out = wrap_path(
        "~/work/some-org/some-umbrella/worktrees/agents-ux/medulla-public",
        28,
        2,
    );

    assert!(
        out.len() <= 2,
        "the path is held to its line budget: {out:?}"
    );
    let joined = out.concat();
    assert!(
        joined.ends_with("medulla-public"),
        "the end of the path survives: {joined:?}"
    );
    assert!(
        joined.starts_with('…'),
        "and the row says it was shortened: {joined:?}"
    );
}

#[test]
fn a_path_segment_of_wide_characters_hard_cuts_by_display_column() {
    // A single unbreakable segment made of two-column characters must still
    // land within the width budget per line, not the char-count budget.
    let out = flow_path("任务一二三四五六七八九十", 10);
    for line in &out {
        assert!(line.width() <= 10, "{line:?} overruns");
    }
    assert_eq!(out.concat(), "任务一二三四五六七八九十");
}

#[test]
fn an_unbreakable_final_segment_keeps_its_own_tail() {
    // When the checkout name itself (the part after the last `/`) is longer
    // than the whole line budget, there is no separator left to drop a head
    // at. The name still has to keep its distinguishing suffix rather than
    // silently losing it to a head-first hard cut, or two harnesses that
    // share a long prefix would render identically.
    let out = wrap_path(
        "~/work/a/very-long-checkout-name-that-alone-overruns-the-budget-abcdefg",
        12,
        2,
    );

    assert!(
        out.len() <= 2,
        "the path is held to its line budget: {out:?}"
    );
    let joined = out.concat();
    assert!(
        joined.ends_with("abcdefg"),
        "the tail of the unbreakable segment survives: {joined:?}"
    );
    assert!(
        joined.starts_with('…'),
        "and the row says it was shortened: {joined:?}"
    );
}

#[test]
fn the_home_directory_collapses_to_a_tilde() {
    // A fixture, not the real environment. Reading `$HOME` here panicked the
    // whole suite on Windows, which sets `USERPROFILE` and no `HOME` — and a
    // presentation rule should not depend on the machine running it anyway.
    let home = Some("/Users/dev");
    assert_eq!(short_home("/Users/dev/work/repo", home), "~/work/repo");
    assert_eq!(short_home("/Users/dev", home), "~");
    assert_eq!(short_home("/Users/dev/", home), "~");
    // A path outside home keeps its head; there is nothing to collapse.
    assert_eq!(short_home("/srv/repos/auth", home), "/srv/repos/auth");
    // A sibling that merely *starts* with the same characters is not inside
    // home, so it must not be rewritten as though it were.
    assert_eq!(short_home("/Users/developer/x", home), "/Users/developer/x");
}

#[test]
fn a_windows_home_collapses_on_its_own_separator() {
    // Windows hands us `C:\Users\dev` while the harness reports whichever
    // separator it was started with, so both have to be recognised.
    let home = Some("C:\\Users\\dev");
    assert_eq!(
        short_home("C:\\Users\\dev\\work\\repo", home),
        "~\\work\\repo"
    );
    assert_eq!(short_home("C:\\Users\\dev", home), "~");
    assert_eq!(short_home("D:\\src\\other", home), "D:\\src\\other");
}

#[test]
fn an_unknown_home_leaves_the_path_alone() {
    // `dirs::home_dir()` can fail. Showing the absolute path is right then —
    // inventing a `~` for a directory we cannot place would be a lie.
    assert_eq!(short_home("/srv/repos/auth", None), "/srv/repos/auth");
    assert_eq!(short_home("/srv/repos/auth", Some("")), "/srv/repos/auth");
}
