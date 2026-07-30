//! Tests for the Agents rail's line layout: the width cap, how a row that does
//! not fit is re-flowed, and what a working directory keeps when it cannot.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as TLine, Span};

use super::rail::{flow_path, short_home, wrap_line, wrap_path};

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
fn the_home_directory_collapses_to_a_tilde() {
    let home = std::env::var("HOME").expect("a home directory");
    assert_eq!(short_home(&format!("{home}/work/repo")), "~/work/repo");
    assert_eq!(short_home(&home), "~");
    assert_eq!(short_home("/srv/repos/auth"), "/srv/repos/auth");
}
