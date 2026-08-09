//! Blocking-error recognition: a usage limit, a dead credential, or an API
//! failure the harness will not retry past.
//!
//! Errors are matched provider-agnostically because they come from the model
//! API rather than any one CLI's chrome.

use medulla::protocol::HarnessProvider;

use super::super::detect::detect;
use super::super::types::AttentionKind;
/// A harness that stopped on a usage limit is as blocked as one asking a
/// question, and nothing on the row said so: the process is alive, the screen is
/// quiet, and the turn simply never produced anything.
#[test]
fn a_blocking_error_is_reported_as_one() {
    let screen = "\
  ⏺ Working on it…

  Claude usage limit reached. Your limit will reset at 3pm.

  > ";
    let (kind, what) = detect(HarnessProvider::Claude, screen).expect("a cue");
    assert_eq!(kind, AttentionKind::Error);
    assert!(what.contains("usage limit reached"), "{what}");
}

/// Errors are matched provider-agnostically because they come from the model
/// API rather than any one CLI's chrome.
#[test]
fn a_blocking_error_is_recognised_whichever_harness_printed_it() {
    for (provider, screen) in [
        (
            HarnessProvider::Codex,
            "You've hit your usage limit.\nPlease try again later.\n> ",
        ),
        (
            HarnessProvider::Claude,
            "OAuth token has expired · Please run /login\n> ",
        ),
    ] {
        let (kind, _) = detect(provider, screen).expect("a cue");
        assert_eq!(kind, AttentionKind::Error, "{screen}");
    }
}

/// An error the operator already dealt with sits in scrollback forever. Only the
/// live tail can be a current one, or a single expired token would leave a row
/// flagged for the rest of the session.
#[test]
fn an_error_scrolled_out_of_the_live_tail_is_not_current() {
    let mut screen = String::from("Invalid API key · Please run /login\n");
    for turn in 0..20 {
        screen.push_str(&format!("  ⏺ did thing {turn}\n"));
    }
    screen.push_str("> ");

    assert_eq!(detect(HarnessProvider::Claude, &screen), None, "{screen}");
}

#[test]
fn an_error_does_not_resurface_after_a_successful_retry() {
    let screen = "Invalid API key\n› retry with the refreshed token\n✓ Retried successfully\n› ";

    assert_eq!(detect(HarnessProvider::Codex, screen), None);
}

#[test]
fn a_draft_error_phrase_does_not_revive_a_recovered_error() {
    let screen = "Invalid API key\n› retry with the refreshed token\n✓ Retried successfully\n› explain invalid API key handling";

    assert_eq!(detect(HarnessProvider::Codex, screen), None);
}

/// A turn that completed is not an error just because its output mentioned an
/// error phrase: the phrase sits above the restored composer with the turn's
/// own output beneath it, so it was part of the transcript rather than the
/// reason the harness stopped.
#[test]
fn a_successful_turn_that_mentions_an_error_phrase_is_not_a_blocking_error() {
    for screen in [
        "authentication failed\n✓ done\n> ",
        "Invalid API key\nFinished in 2.1s\n> ",
    ] {
        assert_eq!(detect(HarnessProvider::Codex, screen), None, "{screen}");
    }
}

/// A terminal error that wraps in a narrow pane is still terminal: the
/// continuation row is part of the message, not evidence of a recovered turn.
#[test]
fn a_wrapped_blocking_error_is_still_blocking() {
    for screen in [
        "Claude usage limit reached.\nYour limit will reset at 3pm.\n> ",
        "Invalid API key.\nPlease run /login to sign in again.\n> ",
    ] {
        let (kind, what) = detect(HarnessProvider::Claude, screen).expect("a cue");
        assert_eq!(kind, AttentionKind::Error, "{screen}");
        assert!(
            what.contains("usage limit") || what.contains("sign-in"),
            "{what}"
        );
    }
}

/// A bullet-prefixed error is still blocking. Providers commonly lead a
/// terminal error or its wrapped instruction with a bullet glyph
/// (`•`, `*`, `·`), so the leading glyph is layout, not proof the harness
/// recovered; the line must not clear the error on that alone.
#[test]
fn a_bullet_prefixed_error_is_still_blocking() {
    for screen in [
        "• Invalid API key · Please run /login\n> ",
        "• You've hit your usage limit. Try again later.\n> ",
        "* Invalid API key · please re-authenticate\n> ",
        "· Invalid API key. Re-authenticate your session.\n> ",
    ] {
        let (kind, what) = detect(HarnessProvider::Claude, screen).expect("a cue");
        assert_eq!(kind, AttentionKind::Error, "{screen}");
        assert!(
            what.contains("usage limit") || what.contains("sign-in"),
            "{what}"
        );
    }
}

/// A terminal error that *names* the failure in recovering vocabulary — "the
/// API key is not working", "the request could not be completed" — is still
/// blocking: the recovery word is negated, so the line describes the failure,
/// not a recovered turn.
#[test]
fn a_negated_recovery_word_is_not_recovery() {
    for screen in [
        "Authentication failed: the API key is not working\n> ",
        "Invalid API key.\nThe request could not be completed.\n> ",
        "Usage limit reached: the task could not be completed.\n> ",
    ] {
        let (kind, what) = detect(HarnessProvider::Claude, screen).expect("a cue");
        assert_eq!(kind, AttentionKind::Error, "{screen}");
        assert!(
            what.contains("usage limit")
                || what.contains("sign-in")
                || what.contains("authentication failed")
                || what.contains("API key"),
            "{what}"
        );
    }
}

/// A genuine recovery line that merely *mentions* a negator further back —
/// "no errors, all resolved" — still counts as recovery: only near-negation
/// inverts a recovery word.
#[test]
fn a_recovery_line_that_mentions_negation_is_still_recovery() {
    let screen = "Invalid API key\nNo errors remained, all resolved.\n> ";
    assert_eq!(detect(HarnessProvider::Claude, screen), None, "{screen}");
}

/// A glyph-led line that carries the full in-flight activity shape — an
/// animated ellipsis or a live elapsed counter — is still recovery evidence.
/// A spinner gerund names no recovery word ("Cogitating" is in no marker list)
/// and its composer below hides it from `is_working`, so only the shaped-glyph
/// branch can clear the error above it: the tightening must not throw away
/// genuine progress lines with the bullet noise.
#[test]
fn an_activity_shaped_bullet_line_is_still_recovery() {
    let screen = "Invalid API key\n✽ Cogitating… (7s)\n> ";
    assert_eq!(detect(HarnessProvider::Claude, screen), None, "{screen}");
}

/// A continuation is only recovery evidence when it carries a live status
/// signal. A sentence about the failure using words that happen to contain a
/// recovery keyword must not clear the error: "unsuccessful" contains
/// "success", but it describes the failure, not a recovered turn.
#[test]
fn a_continuation_that_describes_the_failure_is_not_recovery() {
    let screen = "Invalid API key.\nAuthentication unsuccessful.\n> ";
    let (kind, what) = detect(HarnessProvider::Codex, screen).expect("a cue");
    assert_eq!(kind, AttentionKind::Error, "{screen}");
    assert!(
        what.contains("sign-in") || what.contains("API key"),
        "{what}"
    );
}

/// When two different failures sit in the live tail, the row must describe the
/// *latest* one — the line `error_index` chose — not whichever marker happens
/// to appear first in the table. An older "usage limit reached" above a retry
/// and a newer "Invalid API key" below it must not send the operator back to
/// the quota page.
#[test]
fn the_latest_failure_is_the_one_described() {
    let screen = "Usage limit reached\n> retry with the refreshed token\nInvalid API key · Please run /login\n> ";
    let (kind, what) = detect(HarnessProvider::Codex, screen).expect("a cue");
    assert_eq!(kind, AttentionKind::Error, "{screen}");
    assert!(
        what.contains("sign-in") || what.contains("API key"),
        "{what}"
    );
    assert!(!what.contains("usage limit"), "{what}");
}

/// The final line of a completed turn can itself name an error marker while
/// describing its resolution. The matched line is no more a live error than a
/// continuation row is: "The invalid API key has been replaced." says the
/// problem is over, so the idle session must not turn red.
#[test]
fn a_completed_turn_that_names_the_marker_is_not_a_live_error() {
    for screen in [
        "The invalid API key has been replaced.\n> ",
        "Authentication failed, then succeeded on retry.\n> ",
    ] {
        assert_eq!(detect(HarnessProvider::Codex, screen), None, "{screen}");
    }
}

/// A completed turn whose reply quotes a bare error phrase must not read as a
/// blocking error. Asking the harness to "reply exactly `authentication
/// failed`" puts the marker line exactly where a live error would sit, with no
/// recovery vocabulary and only the restored composer beneath it — the screen
/// cannot tell the two apart, and a missed blink is cheaper than a rail that
/// blinks at nothing.
#[test]
fn a_reply_quoting_a_bare_error_phrase_is_not_blocking() {
    for screen in [
        "> reply exactly \"authentication failed\"\nauthentication failed\n> ",
        "> what did the error say?\nauthentication failed\n> ",
    ] {
        assert_eq!(detect(HarnessProvider::Codex, screen), None, "{screen}");
    }
}

/// A question outranks the error that prompted it: the harness recovered far
/// enough to ask, and the question is the thing the operator can act on.
#[test]
fn a_prompt_outranks_an_error_printed_above_it() {
    let screen = "\
  Invalid API key · Please run /login

  ❯ 1. Yes, and don't ask again
    2. No, and tell Claude what to do differently";
    let (kind, _) = detect(HarnessProvider::Claude, screen).expect("a cue");
    assert_eq!(kind, AttentionKind::Approval);
}

/// Ordinary tool output mentioning failure is not a blocking error. The markers
/// name terminal conditions precisely so a rail does not light up over a test
/// suite printing the word.
#[test]
fn ordinary_error_output_is_not_a_blocking_error() {
    for screen in [
        "error[E0432]: unresolved import\nerror: could not compile\n> ",
        "  2 tests failed, authentication is not the problem\n> ",
        "  Retrying request after error…\n  Working… (esc to interrupt)",
    ] {
        assert_eq!(detect(HarnessProvider::Claude, screen), None, "{screen}");
    }
}

#[test]
fn an_error_phrase_in_a_live_transcript_does_not_stop_work() {
    let screen = "\
> Fix invalid API key handling\n\
✽ Considering… (7s · ↓ 193 tokens · thinking with medium effort)";

    assert_eq!(detect(HarnessProvider::Claude, screen), None);
}

/// Draft text in an idle composer is operator input, not a harness error.
#[test]
fn an_error_phrase_in_an_idle_composer_does_not_stop_the_session() {
    let screen = "> fix invalid API key handling";

    assert_eq!(detect(HarnessProvider::Claude, screen), None);
}
