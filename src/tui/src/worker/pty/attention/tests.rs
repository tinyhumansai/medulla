//! Unit tests for harness attention detection.
//!
//! The screens here are written the way the harnesses draw them — box borders,
//! carets, wrapped lines — because that layout is exactly what the matcher has
//! to survive.

use medulla::tinyplace::HarnessProvider;

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
fn a_working_harness_wants_nothing() {
    assert!(detect(HarnessProvider::Claude, CLAUDE_WORKING).is_none());
    assert!(is_working(CLAUDE_WORKING));
}

#[test]
fn codex_approval_is_recognised() {
    let (kind, what) = detect(HarnessProvider::Codex, CODEX_APPROVAL).expect("a cue");
    assert_eq!(kind, AttentionKind::Approval);
    assert!(what.contains("codex"), "{what}");
}

#[test]
fn a_startup_dialog_outranks_a_prompt() {
    // Codex's trust dialog, which `dialog` also recognises: the cue must carry
    // that module's wording rather than the generic approval one.
    let screen = "Do you trust the contents of this directory?\n› 1. Yes, continue";
    let (kind, what) = detect(HarnessProvider::Codex, screen).expect("a cue");
    assert_eq!(kind, AttentionKind::Dialog);
    assert!(what.contains("trust"), "{what}");
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
fn opencode_permission_wording_is_an_approval() {
    let screen = "opencode wants to run `git push`\n  Allow once   Always allow   Reject";
    let (kind, _) = detect(HarnessProvider::Opencode, screen).expect("a cue");
    assert_eq!(kind, AttentionKind::Approval);
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
fn a_composer_caret_is_not_an_option() {
    // A caret with prose after it is a composer, which is what a harness shows
    // for most of its life; only a caret on a *numbered* option is a menu.
    assert!(!is_selected_option("> "));
    assert!(!is_selected_option("│ >                    │"));
    assert!(!is_selected_option("│ > Try \"fix the failing test\""));
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
