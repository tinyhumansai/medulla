//! Live-work detection: distinguishing a harness mid-turn from one merely
//! alive. A spinner, a gerund and an elapsed timer in the live region mean a
//! turn is in flight; a restored composer ends the live region.

use super::super::detect::{is_idle, is_working};
/// Current Claude Code prints no "esc to interrupt" at all — it draws a spinner,
/// a gerund, and an elapsed timer. Captured from a live session; without this the
/// working veto was dead for every recent Claude, and no row could say a harness
/// was busy rather than merely alive.
#[test]
fn claudes_live_progress_line_counts_as_working() {
    for line in [
        "✽ Considering… (7s · ↓ 193 tokens · thinking with medium effort)",
        "· Considering… (12s · ↓ 568 tokens)",
        "* Cogitating… (5s · ↓ 193 tokens · thought for 1s)",
        "✻ Cogitated for 5s… (45s · 1 shell still running)",
    ] {
        // No composer or phrase marker: the progress line alone carries this
        // verdict.
        let screen = format!("  ⏺ reading files\n{line}\n  bypass permissions on");
        assert!(is_working(&screen), "{line}");
    }
}

/// A restored idle composer ends the live region, even if the final spinner
/// line remains close by in Claude's scrollback.
#[test]
fn a_progress_line_above_an_idle_composer_is_not_working() {
    let screen = "✽ Considering… (7s · ↓ 193 tokens)\n❯ ";

    assert!(!is_working(screen));
}

/// The composer placeholder Claude swaps in for the duration of a turn.
#[test]
fn claudes_queued_message_hint_counts_as_working() {
    assert!(is_working(
        "  ⏺ working\n❯ Press up to edit queued messages\n  medulla-public Opus 5"
    ));
}

/// A retained interrupt footer above an ordinary composer is not live work.
#[test]
fn a_retained_working_marker_above_an_idle_composer_is_not_working() {
    assert!(!is_working("esc to interrupt\n❯ Write a message"));
}

/// Codex has not changed, and must not be broken by teaching the matcher Claude.
#[test]
fn codex_still_announces_its_turn_the_old_way() {
    assert!(is_working(
        "• Working (8s • esc to interrupt)\n› Find and fix a bug"
    ));
}

/// A draft in the idle composer that echoes a footer phrase is operator input,
/// not the harness working: it must not spin the row or veto real cues until
/// the draft clears.
#[test]
fn a_draft_echoing_a_working_phrase_in_the_composer_is_not_working() {
    assert!(!is_working("› document esc to interrupt behavior"));
}

/// Each part of the progress line appears in ordinary output on its own. Only
/// the three together mean a turn is in flight.
#[test]
fn progress_line_lookalikes_are_not_working() {
    for screen in [
        // A retained progress line, scrolled well above the live composer.
        &format!(
            "✽ Considering… (7s · ↓ 12 tokens)\n{}\n> Write a message",
            "  ⏺ done\n".repeat(10)
        ),
        // Elapsed timer, no spinner glyph and no ellipsis.
        "  Ran tests (12s)\n> Write a message",
        // Spinner glyph and ellipsis, no timer — Claude's idle bullet list.
        "· Loading…\n> Write a message",
        // Parenthesised digits that are not an elapsed timer.
        "✽ Considering… (2s3 build)\n> Write a message",
    ]
    .map(String::from)
    {
        assert!(!is_working(&screen), "{screen}");
    }
}

#[test]
fn a_live_composer_is_explicit_idle_evidence() {
    assert!(is_idle("last answer\n❯ Write a message\n  ? for shortcuts"));
    assert!(is_idle("last answer\n> "));
    assert!(!is_idle("✽ Considering… (7s · ↓ 193 tokens)"));
    assert!(!is_idle("Do you want to proceed?\n❯ 1. Yes\n  2. No"));
}
