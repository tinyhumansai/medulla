//! Prompt, dialog and choice recognition: the distinctive menus each harness
//! paints when it is asking the operator something.

use medulla::protocol::HarnessProvider;

use super::super::detect::{detect, is_selected_option, is_working};
use super::super::types::AttentionKind;
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
fn current_claude_permission_prompt_outranks_its_waiting_tool_line() {
    let screen = "● Bash(touch claude-state-probe.txt)\n\
                    ⎿  Waiting…\n\
                  Bash command\n\
                    touch claude-state-probe.txt\n\
                  Do you want to proceed?\n\
                  ❯ 1. Yes\n\
                    2. Yes, and always allow access to work-state/ from this project\n\
                    3. No\n\
                  Esc to cancel";

    assert!(
        is_working(screen),
        "the tool status alone resembles progress"
    );
    let (kind, what) = detect(HarnessProvider::Claude, screen).expect("a cue");
    assert_eq!(kind, AttentionKind::Approval);
    assert_eq!(what, "claude is asking permission");
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

/// OpenCode's permission labels are too generic for the marker table, so they
/// are matched structurally instead. The same scrollback rule applies: an
/// answered menu's retained labels above the restored composer must not
/// recreate the approval cue on every poll.
#[test]
fn an_answered_opencode_permission_menu_above_an_idle_composer_is_not_an_approval() {
    let screen = "  Allow once   Always allow   Reject\n  ✓ Ran the command\n  > Try \"fix the failing test\"";
    assert_eq!(detect(HarnessProvider::Opencode, screen), None);
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
