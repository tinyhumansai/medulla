//! Tests for the shared chat surface.
//!
//! These assert on rendered cells rather than on the returned line structs,
//! because the bugs this module exists to fix were all about what reached the
//! screen: a caret that was drawn on the wrong row, a placeholder that survived
//! focus, a scroll offset that banked presses the viewport ignored.

use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::text::{Line as TLine, Span};
use ratatui::Terminal;

use crate::ui::composer::Draft;
use crate::ui::theme::Theme;

use super::types::TranscriptFit;
use super::{composer_height, draw_composer, draw_transcript, ComposerChrome};

/// Render `draw` into a fresh `width`×`height` terminal and return its rows.
fn rows(width: u16, height: u16, draw: impl FnOnce(&mut ratatui::Frame, Rect)) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
        .draw(|f| {
            let area = f.area();
            draw(f, area)
        })
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

/// A draft whose caret sits after `text`.
fn draft(text: &str) -> Draft {
    Draft {
        cursor: text.chars().count(),
        text: text.to_string(),
    }
}

#[test]
fn a_composer_grows_with_its_draft_rather_than_capping_it() {
    assert_eq!(composer_height(""), 3);
    assert_eq!(composer_height("one line"), 3);
    assert_eq!(composer_height("one\ntwo\nthree"), 5);
    // Well past any pane's own cap: the height is the caller's to clamp, and a
    // widget that lied about it would hide the end of what is being typed.
    assert_eq!(composer_height(&"x\n".repeat(20)), 23);
}

#[test]
fn the_draft_is_prompted_once_and_continuations_are_indented() {
    let out = rows(24, 5, |f, area| {
        draw_composer(
            f,
            area,
            &draft("first\nsecond"),
            &Theme::default(),
            ComposerChrome::default(),
        )
    });
    assert!(out[1].contains("❯ first"), "{out:?}");
    // Not a second `❯`: a continuation row carrying the prompt reads as a new
    // message rather than the same one.
    assert!(out[2].contains("  second"), "{out:?}");
    assert!(!out[2].contains('❯'), "{out:?}");
}

#[test]
fn the_caret_lands_on_the_row_it_is_actually_on() {
    // The bug this replaces: a single-line helper rendered a multi-line draft
    // onto one row, so the caret appeared on the first line no matter where the
    // cursor was.
    let out = rows(24, 5, |f, area| {
        draw_composer(
            f,
            area,
            &draft("first\nsecond"),
            &Theme::default(),
            ComposerChrome {
                focused: true,
                ..ComposerChrome::default()
            },
        )
    });
    assert!(out[2].contains("second"), "{out:?}");
}

#[test]
fn a_placeholder_shows_only_on_an_empty_unfocused_draft() {
    let chrome = |focused| ComposerChrome {
        focused,
        busy: false,
        placeholder: Some("Ask for a change"),
    };

    let idle = rows(32, 3, |f, area| {
        draw_composer(f, area, &Draft::default(), &Theme::default(), chrome(false))
    });
    assert!(idle[1].contains("Ask for a change"), "{idle:?}");

    // Focused: the caret is the invitation, and a placeholder under it reads as
    // text that is already in the draft.
    let focused = rows(32, 3, |f, area| {
        draw_composer(f, area, &Draft::default(), &Theme::default(), chrome(true))
    });
    assert!(!focused[1].contains("Ask for a change"), "{focused:?}");

    // A draft that has something in it always wins over the placeholder.
    let typed = rows(32, 3, |f, area| {
        draw_composer(f, area, &draft("hello"), &Theme::default(), chrome(false))
    });
    assert!(typed[1].contains("hello"), "{typed:?}");
    assert!(!typed[1].contains("Ask for a change"), "{typed:?}");
}

#[test]
fn a_composer_too_small_to_draw_does_not_panic() {
    for (w, h) in [(0, 0), (1, 1), (2, 2), (40, 1)] {
        let _ = rows(w.max(1), h.max(1), |f, area| {
            draw_composer(
                f,
                Rect {
                    width: w,
                    height: h,
                    ..area
                },
                &draft("x"),
                &Theme::default(),
                ComposerChrome::default(),
            )
        });
    }
}

/// `count` numbered lines, so a viewport assertion can name which ones it got.
fn numbered(count: usize) -> Vec<TLine<'static>> {
    (0..count)
        .map(|i| TLine::from(Span::raw(format!("line{i}"))))
        .collect()
}

#[test]
fn a_transcript_is_anchored_to_its_end() {
    let out = rows(16, 3, |f, area| {
        draw_transcript(f, area, numbered(10), 0);
    });
    // The newest turn is the one being waited on, so it is the one on screen.
    assert_eq!(out, vec!["line7", "line8", "line9"]);
}

#[test]
fn scrolling_back_walks_towards_the_start() {
    let out = rows(16, 3, |f, area| {
        draw_transcript(f, area, numbered(10), 2);
    });
    assert_eq!(out, vec!["line5", "line6", "line7"]);
}

#[test]
fn a_scroll_past_the_start_is_clamped_rather_than_banked() {
    // The reported offset is what the caller must store. Saturating the start
    // index instead would let a held PageUp accumulate an offset the viewport
    // ignores, so PageDown would then do nothing until every one of those
    // presses had been given back.
    let mut fit = TranscriptFit::default();
    let out = rows(16, 3, |f, area| {
        fit = draw_transcript(f, area, numbered(10), 999);
    });
    assert_eq!(out, vec!["line0", "line1", "line2"]);
    assert_eq!(fit.scroll, 7);
    assert_eq!(fit.below, 7);
}

#[test]
fn content_that_fits_cannot_be_scrolled_at_all() {
    let mut fit = TranscriptFit::default();
    let out = rows(16, 5, |f, area| {
        fit = draw_transcript(f, area, numbered(2), 4);
    });
    assert_eq!(
        fit,
        TranscriptFit {
            scroll: 0,
            below: 0
        }
    );
    assert_eq!(out[0], "line0");
    assert_eq!(out[1], "line1");
}

#[test]
fn a_transcript_with_no_room_reports_nothing_and_does_not_panic() {
    let mut fit = TranscriptFit {
        scroll: 3,
        below: 3,
    };
    let _ = rows(16, 1, |f, area| {
        fit = draw_transcript(f, Rect { height: 0, ..area }, numbered(10), 3);
    });
    assert_eq!(fit, TranscriptFit::default());
}
