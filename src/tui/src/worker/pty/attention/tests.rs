//! Unit tests for harness attention detection.
//!
//! The screens here are written the way the harnesses draw them — box borders,
//! carets, wrapped lines — because that layout is exactly what the matcher has
//! to survive.

use medulla::protocol::HarnessProvider;

use super::detect::{detect, is_selected_option, is_working};
use super::types::{AttentionKind, HarnessAttention};

/// Claude's tool-permission prompt, as the CLI paints it.
const CLAUDE_PERMISSION: &str = "\
╭──────────────────────────────────────────────╮
│ Bash command                                 │
│                                              │
│   rm -rf build                               │
│   Remove the build directory                 │
│                                              │
│ Do you want to proceed?                      │
│ ❯ 1. Yes                                     │
│   2. Yes, and don't ask again for rm commands│
│   3. No, and tell Claude what to do          │
│      differently (esc)                       │
╰──────────────────────────────────────────────╯";

/// Claude mid-turn: a spinner and the interrupt footer, nothing being asked.
const CLAUDE_WORKING: &str = "\
✻ Reticulating splines… (12s · esc to interrupt)

  > try again";

/// Codex's command approval.
const CODEX_APPROVAL: &str = "\
  Allow Codex to run `cargo test`?

  › 1. Yes, proceed
    2. Yes, and don't ask again
    3. No, and tell Codex what to do differently";

#[test]
fn claude_permission_prompt_is_an_approval() {
    let (kind, what) = detect(HarnessProvider::Claude, CLAUDE_PERMISSION).expect("a cue");
    assert_eq!(kind, AttentionKind::Approval);
    assert!(what.contains("claude"), "{what}");
}

#[test]
fn claude_plan_prompt_names_planning() {
    let screen =
        "Would you like to proceed?\n❯ 1. Yes, and auto-accept edits\n  3. No, keep planning";
    let (kind, what) = detect(HarnessProvider::Claude, screen).expect("a cue");
    assert_eq!(kind, AttentionKind::Approval);
    assert!(what.contains("planning"), "{what}");
}

#[test]
fn a_wrapped_claude_plan_prompt_still_names_planning() {
    let screen = "Would you like to proceed?\n\
                  ❯ 1. Yes, and auto-\n\
                       accept edits\n\
                    2. Yes, but review\n\
                       each edit first\n\
                    3. No, keep\n\
                       planning";
    let (kind, what) = detect(HarnessProvider::Claude, screen).expect("a cue");
    assert_eq!(kind, AttentionKind::Approval);
    assert!(what.contains("planning"), "{what}");
}

#[test]
fn ordinary_keep_planning_words_are_not_a_plan_prompt() {
    for screen in [
        "User: keep planning while you investigate",
        "User: keep planning while you investigate\nWorking… (esc to interrupt)",
    ] {
        assert!(detect(HarnessProvider::Claude, screen).is_none());
    }
}

#[test]
fn a_retained_plan_option_does_not_relabel_a_new_menu() {
    let screen = "Earlier plan menu:\n\
                    1. Keep planning\n\
                  Pick a model to continue with:\n\
                  ❯ 1. Sonnet\n\
                    2. Opus";
    let (kind, what) = detect(HarnessProvider::Claude, screen).expect("a cue");
    assert_eq!(kind, AttentionKind::Choice);
    assert!(!what.contains("planning"), "{what}");
}

#[test]
fn a_working_harness_wants_nothing() {
    assert!(detect(HarnessProvider::Claude, CLAUDE_WORKING).is_none());
    assert!(is_working(CLAUDE_WORKING));
}

#[test]
fn retained_plan_options_do_not_override_active_work() {
    let screen = "Earlier plan menu:\n\
                  ❯ 1. Yes, and auto-accept edits\n\
                    2. Yes, and manually approve edits\n\
                  ✻ Implementing… (12s · esc to interrupt)\n\
                    >";

    assert!(detect(HarnessProvider::Claude, screen).is_none());
}

#[test]
fn a_plan_menu_above_an_idle_composer_is_not_current() {
    let screen = "Would you like to proceed?\n\
                  ❯ 1. Yes, and auto-accept edits\n\
                    3. No, keep planning\n\
                    >";

    assert!(detect(HarnessProvider::Claude, screen).is_none());
}

#[test]
fn codex_approval_is_recognised() {
    let (kind, what) = detect(HarnessProvider::Codex, CODEX_APPROVAL).expect("a cue");
    assert_eq!(kind, AttentionKind::Approval);
    assert!(what.contains("codex"), "{what}");
}

#[test]
fn ordinary_yes_proceed_text_does_not_interrupt_a_working_codex() {
    let screen = "User: yes, proceed with the refactor\nWorking… (esc to interrupt)";
    assert!(detect(HarnessProvider::Codex, screen).is_none());
}

#[test]
fn ordinary_allow_codex_text_does_not_interrupt_a_working_codex() {
    let screen = "User: allow Codex to finish the refactor\nWorking… (esc to interrupt)";
    assert!(detect(HarnessProvider::Codex, screen).is_none());
}

#[test]
fn ordinary_codex_wants_text_does_not_interrupt_a_working_codex() {
    let screen = "Assistant: Codex wants to update these files\nWorking… (esc to interrupt)";
    assert!(detect(HarnessProvider::Codex, screen).is_none());
}

#[test]
fn an_ordinary_question_does_not_interrupt_a_working_claude() {
    let screen =
        "Assistant: Do you want to proceed with the refactor?\nWorking… (esc to interrupt)";
    assert!(detect(HarnessProvider::Claude, screen).is_none());
}

#[test]
fn a_startup_dialog_outranks_a_prompt() {
    // Codex's trust dialog, which `dialog` also recognises: the cue must carry
    // that module's wording rather than the generic approval one.
    let screen = "Do you trust the contents of this directory?\n\
                  › 1. Yes, continue\n\
                    2. No, quit\n\n\
                  Press enter to continue";
    let (kind, what) = detect(HarnessProvider::Codex, screen).expect("a cue");
    assert_eq!(kind, AttentionKind::Dialog);
    assert!(what.contains("trust"), "{what}");
}

#[test]
fn retained_startup_dialog_words_do_not_interrupt_active_work() {
    for screen in [
        "Assistant: trust the contents of this directory\nWorking… (esc to interrupt)",
        "Assistant: choose skip until next version\nWorking… (esc to interrupt)",
    ] {
        assert!(detect(HarnessProvider::Codex, screen).is_none());
    }
}

#[test]
fn retained_startup_dialog_above_a_composer_is_not_active() {
    let screen = "Earlier: Do you trust the contents of this directory?\n\
                  › 1. Yes, continue\n\
                  Press enter to continue\n\n\
                  > Try \"fix the failing test\"\n\
                  ? for shortcuts";
    assert!(detect(HarnessProvider::Codex, screen).is_none());
}

#[test]
fn an_unrecognised_menu_is_still_a_choice() {
    // Wording no table knows, shape every harness shares.
    let screen = "Pick a model to continue with:\n  ❯ 1. gpt-5-codex\n    2. o3";
    let (kind, what) = detect(HarnessProvider::Opencode, screen).expect("a cue");
    assert_eq!(kind, AttentionKind::Choice);
    assert!(what.contains("opencode"), "{what}");
}

#[test]
fn a_yes_no_confirmation_is_a_choice() {
    let (kind, _) =
        detect(HarnessProvider::Opencode, "Overwrite config.toml? (y/n)").expect("a cue");
    assert_eq!(kind, AttentionKind::Choice);
}

#[test]
fn a_retained_yes_no_exchange_above_the_composer_is_not_a_choice() {
    let screen = "Overwrite config.toml? (y/n)\naccepted\n\n> Try \"fix the failing test\"";
    assert!(detect(HarnessProvider::Opencode, screen).is_none());
}

#[test]
fn opencode_permission_wording_is_an_approval() {
    let screen = "opencode wants to run `git push`\n  Allow once   Always allow   Reject";
    let (kind, _) = detect(HarnessProvider::Opencode, screen).expect("a cue");
    assert_eq!(kind, AttentionKind::Approval);
}

#[test]
fn one_generic_opencode_permission_phrase_is_not_an_approval() {
    for phrase in ["always allow retries", "permission required for this file"] {
        let screen = format!("Assistant: {phrase}\nWorking… (esc to interrupt)");
        assert!(detect(HarnessProvider::Opencode, &screen).is_none());
    }
}

#[test]
fn an_idle_composer_is_not_a_question() {
    // The single most important negative: this is what every harness shows for
    // most of its life, and blinking through it would train the operator to
    // ignore the rail.
    let screen = "\
╭──────────────────────────────────────────╮
│ > Try \"fix the failing test\"            │
╰──────────────────────────────────────────╯
  ? for shortcuts";
    assert!(detect(HarnessProvider::Claude, screen).is_none());
    assert!(detect(HarnessProvider::Codex, screen).is_none());
    assert!(detect(HarnessProvider::Opencode, screen).is_none());
}

#[test]
fn a_retained_numbered_menu_above_the_composer_is_not_a_choice() {
    let screen =
        "Pick a model:\n❯ 1. fast\n  2. careful\n\n> Try \"ask another question\"\n? for shortcuts";
    assert!(detect(HarnessProvider::Codex, screen).is_none());
}

/// A permission menu the operator already answered stays in scrollback above
/// the restored composer; its marker labels must not recreate the approval cue.
#[test]
fn an_answered_permission_menu_above_an_idle_composer_is_not_an_approval() {
    let screen = "  ❯ 1. Yes\n    2. Yes, and don't ask again\n    3. No, and tell Claude what to do\n  ✓ Ran the command\n  > ";
    assert_eq!(detect(HarnessProvider::Claude, screen), None);
}

#[test]
fn a_composer_caret_is_not_an_option() {
    // A caret with prose after it is a composer, which is what a harness shows
    // for most of its life; only a caret on a *numbered* option is a menu.
    assert!(!is_selected_option("> "));
    assert!(!is_selected_option("│ >                    │"));
    assert!(!is_selected_option("│ > Try \"fix the failing test\""));
    assert!(!is_selected_option("> 1. update the parser"));
    assert!(!is_selected_option("  › 1"));
    assert!(is_selected_option("  ❯ 1. Yes"));
    assert!(is_selected_option("│ › 3) Skip until next version"));
}

#[test]
fn a_named_cue_outranks_a_bell() {
    let bell = HarnessAttention::new(AttentionKind::Bell, "rang", 0);
    let approval = HarnessAttention::new(AttentionKind::Approval, "asking", 10);
    assert!(approval.supersedes(&bell));
    assert!(!bell.supersedes(&approval));
}

#[test]
fn the_same_cue_keeps_its_first_seen_time() {
    let first = HarnessAttention::new(AttentionKind::Approval, "asking", 100);
    let again = HarnessAttention::new(AttentionKind::Approval, "asking", 900);
    // Equal kinds do not displace, which is what preserves `since` — a prompt
    // repainted every frame must not read as newly arrived every frame.
    assert!(!again.supersedes(&first));
}

#[test]
fn the_label_states_how_long_it_has_waited() {
    let cue = HarnessAttention::new(
        AttentionKind::Approval,
        "claude is asking permission",
        1_000,
    );
    assert_eq!(cue.label(1_400), "claude is asking permission");
    assert_eq!(cue.label(13_000), "claude is asking permission · 12s");
}

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
        (HarnessProvider::Codex, "You've hit your usage limit.\n> "),
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

/// The plan-exit menu names planning rather than falling back to the generic
/// permission wording, even when the pane is too narrow for the structural walk
/// to rejoin "keep planning" across its wrap.
#[test]
fn the_plan_exit_menu_is_named_by_its_accept_options() {
    let screen =
        "Ready to code?\n❯ 1. Yes, and auto-accept edits\n  2. Yes, and manually approve edits";
    let (kind, what) = detect(HarnessProvider::Claude, screen).expect("a cue");
    assert_eq!(kind, AttentionKind::Approval);
    assert!(what.contains("planning"), "{what}");
}

/// Precedence across the whole vocabulary, in one place: a cue that names a
/// harder fact must never be displaced by a vaguer one.
#[test]
fn the_cue_vocabulary_is_ordered_from_certain_to_vague() {
    let kinds = [
        AttentionKind::Failed,
        AttentionKind::Dialog,
        AttentionKind::Approval,
        AttentionKind::Error,
        AttentionKind::Choice,
        AttentionKind::Completed,
        AttentionKind::Bell,
    ];
    for window in kinds.windows(2) {
        let stronger = HarnessAttention::new(window[0], "a", 0);
        let weaker = HarnessAttention::new(window[1], "b", 0);
        assert!(
            stronger.supersedes(&weaker),
            "{:?} must outrank {:?}",
            window[0],
            window[1]
        );
        assert!(!weaker.supersedes(&stronger));
    }
}

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
