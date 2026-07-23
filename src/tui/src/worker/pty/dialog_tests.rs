//! Tests for [`super::dialog`] — recognising a harness blocked on a startup
//! dialog.
//!
//! Split from `tests.rs`, which sits at the file-size ceiling, and which is
//! about driving real ptys; these are about one pure function over screen text.

/// Claude Code's workspace trust dialog, captured verbatim from the installed
/// CLI on a pty in an untrusted directory.
///
/// Kept as one literal rather than paraphrased: this text is the contract, and
/// a paraphrase that drifts from what the harness actually paints would leave
/// the detector passing its tests while failing in front of an operator.
const CLAUDE_TRUST_DIALOG: &str = "\
Accessing workspace:
/private/var/folders/h3/567qvkf514v4flrv5p80t1sc0000gn/T/trustprobe-8w_rfiru

Quick safety check: Is this a project you created or one you trust? (Like your
own code, a well-known open source project, or work from your team). If not,
take a moment to review what's in this folder first.

Claude Code'll be able to read, edit, and execute files here.

Security guide
> 1. Yes, I trust this folder
  2. No, exit

Enter to confirm - Esc to cancel";

#[test]
fn the_workspace_trust_dialog_is_recognised() {
    let dialog = super::dialog::blocking_dialog(CLAUDE_TRUST_DIALOG)
        .expect("claude's trust dialog must be recognised");
    assert!(dialog.what.contains("approve this workspace"), "{dialog:?}");
    assert!(
        dialog.remedy.contains("startup"),
        "the remedy must point at the preflight that should have handled it: {dialog:?}"
    );
}

#[test]
fn a_dialog_is_recognised_however_the_harness_laid_it_out() {
    // A full-screen TUI positions text with cursor moves, so where it put its
    // spaces and line breaks is not something to depend on.
    let squeezed = CLAUDE_TRUST_DIALOG.replace([' ', '\n'], "");
    assert!(super::dialog::blocking_dialog(&squeezed).is_some());
    let shouty = CLAUDE_TRUST_DIALOG.to_uppercase();
    assert!(super::dialog::blocking_dialog(&shouty).is_some());
}

#[test]
fn an_ordinary_composer_is_not_mistaken_for_a_dialog() {
    // A false positive would refuse a turn the harness was ready to run, which
    // is worse than the timeout this exists to replace.
    for screen in [
        "",
        "> Try \"fix the failing test\"\n? for shortcuts",
        "Welcome to Claude Code\n\n> ",
        // Prose that merely discusses trust must not trip it.
        "I reviewed the auth module and the trust boundary looks correct.",
    ] {
        assert_eq!(
            super::dialog::blocking_dialog(screen),
            None,
            "false positive on: {screen:?}"
        );
    }
}

/// Claude's bypass-permissions disclaimer, captured verbatim from the installed
/// CLI running `claude --dangerously-skip-permissions`.
///
/// Note the selected option: this dialog defaults to **exit**, which is why
/// typing blind at it is worse than at any other.
const CLAUDE_BYPASS_DISCLAIMER: &str = "\
 WARNING: Claude Code running in Bypass Permissions mode

 In Bypass Permissions mode, Claude Code will not ask for your approval
 before running potentially dangerous commands.
 This mode should only be used in a sandboxed container/VM that has
 restricted internet access and can easily be restored if damaged.
 By proceeding, you accept all responsibility for actions taken while running
 in Bypass Permissions mode.

 https://code.claude.com/docs/en/security

 > 1. No, exit
   2. Yes, I accept

 Enter to confirm - Esc to cancel";

#[test]
fn the_bypass_permissions_disclaimer_is_recognised() {
    let dialog = super::dialog::blocking_dialog(CLAUDE_BYPASS_DISCLAIMER)
        .expect("the disclaimer must be recognised");
    assert!(dialog.what.contains("bypass-permissions"), "{dialog:?}");
}

#[test]
fn the_two_dialogs_are_told_apart() {
    // They call for different remedies, and reporting one as the other would
    // send an operator looking for a prompt that is not on screen.
    let trust = super::dialog::blocking_dialog(CLAUDE_TRUST_DIALOG).unwrap();
    let bypass = super::dialog::blocking_dialog(CLAUDE_BYPASS_DISCLAIMER).unwrap();
    assert_ne!(trust, bypass);
    assert!(trust.what.contains("workspace"));
}
