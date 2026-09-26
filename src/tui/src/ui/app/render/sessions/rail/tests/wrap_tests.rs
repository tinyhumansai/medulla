//! Tests for wrapping Sessions rail rows and compacting their paths.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span};
use unicode_width::UnicodeWidthStr;

use super::super::wrap::{flow_path, short_home, wrap_line, wrap_path};

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
    let second: String = out[1]
        .spans
        .iter()
        .map(|span| span.content.to_string())
        .collect();
    assert!(second.starts_with("     "), "continuation: {second:?}");
}

#[test]
fn wrapping_keeps_each_span_style() {
    let coloured = Style::default().add_modifier(Modifier::BOLD);
    let line = TLine::from(vec![
        Span::raw("   task-1 aaaa bbbb cccc "),
        Span::styled("running", coloured),
        Span::raw(" · 9 turns"),
    ]);
    let out = wrap_line(&line, 20, 5);
    let kept: usize = out
        .iter()
        .flat_map(|line| line.spans.iter())
        .filter(|span| span.style == coloured)
        .map(|span| span.content.chars().count())
        .sum();
    assert_eq!(kept, "running".len());
}

#[test]
fn wide_characters_wrap_by_display_column_not_char_count() {
    let out = wrap_line(&TLine::from(Span::raw("任务一二三四五六七八九十")), 10, 0);
    assert!(out.len() > 1);
    assert!(out.iter().all(|line| line.width() <= 10));
}

#[test]
fn a_path_breaks_on_separators() {
    let out = flow_path("~/work/tinyhumans/medulla", 12);
    assert!(out.iter().all(|line| line.chars().count() <= 12));
    assert_eq!(out.concat(), "~/work/tinyhumans/medulla");
}

#[test]
fn a_path_too_long_to_fit_keeps_its_tail() {
    let limited = wrap_path(
        "~/work/some-org/some-umbrella/worktrees/agents-ux/medulla-public",
        28,
        2,
    );
    assert!(limited.len() <= 2);
    assert!(limited.concat().ends_with("medulla-public"));
    assert!(limited.concat().starts_with('…'));
}

#[test]
fn a_path_segment_of_wide_characters_hard_cuts_by_display_column() {
    let wide = flow_path("任务一二三四五六七八九十", 10);
    assert!(wide.iter().all(|line| line.width() <= 10));
    assert_eq!(wide.concat(), "任务一二三四五六七八九十");
}

#[test]
fn an_unbreakable_final_segment_keeps_its_own_tail() {
    let long = wrap_path(
        "~/work/a/very-long-checkout-name-that-alone-overruns-the-budget-abcdefg",
        12,
        2,
    );
    assert!(long.len() <= 2);
    assert!(long.concat().ends_with("abcdefg"));
    assert!(long.concat().starts_with('…'));
}

#[test]
fn homes_compact_only_their_own_path_prefix() {
    let home = Some("/Users/dev");
    assert_eq!(short_home("/Users/dev/work/repo", home), "~/work/repo");
    assert_eq!(short_home("/Users/dev", home), "~");
    assert_eq!(short_home("/Users/developer/x", home), "/Users/developer/x");
    assert_eq!(short_home("/srv/repos/auth", None), "/srv/repos/auth");
}

#[test]
fn a_windows_home_collapses_on_its_own_separator() {
    let home = Some("C:\\Users\\dev");
    assert_eq!(
        short_home("C:\\Users\\dev\\work\\repo", home),
        "~\\work\\repo"
    );
    assert_eq!(short_home("D:\\src\\other", home), "D:\\src\\other");
}
