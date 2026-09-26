//! Live-work detection: distinguishing a harness mid-turn from one merely
//! alive. A spinner, a gerund and an elapsed timer in the live region mean a
//! turn is in flight; a restored composer ends the live region.

use super::super::detect::is_working;
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

/// A composer below the spinner does *not* end the live region. Claude 2.1
/// keeps its prompt drawn for the whole turn, so this is the ordinary shape of
/// a working pane rather than a spinner left over above a restored composer.
#[test]
fn a_progress_line_above_a_composer_is_still_working() {
    assert!(is_working("✽ Considering… (7s · ↓ 193 tokens)\n❯ "));
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

/// A pane resting at its prompt is not working — no spinner, whatever else is
/// on it.
#[test]
fn a_pane_with_no_spinner_is_not_working() {
    assert!(!is_working(
        "last answer\n❯ Write a message\n  ? for shortcuts"
    ));
    assert!(!is_working("last answer\n> "));
    assert!(!is_working("Do you want to proceed?\n❯ 1. Yes\n  2. No"));
}

/// The bottom of a real Claude 2.1 pane, captured mid-turn.
///
/// The composer is not taken away while Claude works: it stays drawn, framed by
/// its rules, with the status line and the mode hint below it and a tip line
/// wedged in under the spinner. The live progress line therefore sits several
/// rows above the bottom of the pane, behind a composer — which the older scan,
/// stopping at the first composer it met from the bottom, could never reach.
/// Every working turn read as idle, so no row ever spun.
const CLAUDE_TURN_IN_FLIGHT: &str = concat!(
    "⏺ Read(sort.py)\n",
    "  ⎿  Read 42 lines\n",
    "✶ Pondering… (41s · ↓ 2.6k tokens)\n",
    "  ⎿  Tip: Use /btw to ask a quick side question without interrupting Claude's current work\n",
    "                                                        ◐ medium · /effort\n",
    "────────────────────────────────────────\n",
    "❯ \n",
    "────────────────────────────────────────\n",
    "  probe Opus 5 (1M context) | 95% left                /rc\n",
    "  ⏵⏵ accept edits on (shift+tab to cycle) · ← for agents\n",
);

#[test]
fn claude_working_behind_its_own_composer_is_working() {
    assert!(is_working(CLAUDE_TURN_IN_FLIGHT));
}

/// A draft typed into the composer while Claude works does not end the turn.
#[test]
fn a_draft_in_the_composer_does_not_end_a_live_turn() {
    let screen = CLAUDE_TURN_IN_FLIGHT.replace("❯ \n", "❯ benchmark it against sorted()\n");

    assert!(is_working(&screen));
}

/// The same pane once the turn settles. Claude rewrites the spinner in place
/// into a "done" line that keeps the glyph and drops both the ellipsis and the
/// live timer, so the identical layout reads as idle.
#[test]
fn claudes_settled_pane_is_not_working() {
    let screen = CLAUDE_TURN_IN_FLIGHT
        .replace("✶ Pondering… (41s · ↓ 2.6k tokens)", "✻ Sautéed for 48s · done 6:06 PM")
        .replace(
            "  ⎿  Tip: Use /btw to ask a quick side question without interrupting Claude's current work\n",
            "",
        );

    assert!(!is_working(&screen));
}
