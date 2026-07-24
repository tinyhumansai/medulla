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

#[test]
fn claudes_modals_are_report_only() {
    // The worker will not answer claude's startup modals by keypress: they are
    // gated by config the preflight pre-accepts, so meeting one means that
    // failed, and the fix is the config, not a blind Return into the modal.
    for screen in [CLAUDE_TRUST_DIALOG, CLAUDE_BYPASS_DISCLAIMER] {
        let dialog = super::dialog::blocking_dialog(screen).unwrap();
        assert!(
            dialog.dismissal.is_none(),
            "claude's modals must be report-only: {dialog:?}"
        );
    }
}

/// Codex's trust prompt, captured from codex-cli 0.142.5 on a fresh workspace —
/// the dialog that actually blocks the worker, shown even under the bypass flag.
///
/// Kept close to what codex paints so the marker is exercised against realistic
/// text. Its default is `› 1. Yes, continue`, which is why Enter dismisses it.
const CODEX_TRUST_DIALOG: &str = "\
> You are in /private/var/folders/h3/T/.tmpiskvLA

  Do you trust the contents of this directory? Working with untrusted contents
  comes with higher risk of prompt injection. Trusting the directory allows
  project-local config, hooks, and exec policies to load.

› 1. Yes, continue
  2. No, quit

  Press enter to continue";

#[test]
fn codexs_trust_dialog_is_recognised_and_dismissed_with_enter() {
    let dialog = super::dialog::blocking_dialog(CODEX_TRUST_DIALOG)
        .expect("codex's trust dialog must be recognised");
    assert!(dialog.what.contains("trust this workspace"), "{dialog:?}");
    // The default is already highlighted, so a single Enter confirms it — no
    // arrow navigation that could land on "No, quit", and never Esc (which
    // cancels the dialog and exits codex, verified live against 0.142.5).
    let keys = dialog
        .dismissal
        .expect("the trust dialog must carry a safe dismissal");
    assert_eq!(
        keys,
        &[b"\r".as_slice()],
        "trust is confirmed with a lone Enter, nothing else: {keys:?}"
    );
}

#[test]
fn codexs_trust_dialog_is_told_apart_from_claudes() {
    // Both are "trust" dialogs but answered differently — codex's default is
    // Yes-continue (Enter is safe), claude's is config-gated (report-only) — so
    // they must never be confused.
    let codex = super::dialog::blocking_dialog(CODEX_TRUST_DIALOG).unwrap();
    let claude = super::dialog::blocking_dialog(CLAUDE_TRUST_DIALOG).unwrap();
    assert_ne!(codex, claude);
    assert!(claude.dismissal.is_none(), "claude's trust is report-only");
}

/// Codex's *interactive* update prompt, in the three-option layout with the
/// cursor resting on the dangerous `1. Update now` default.
const CODEX_UPDATE_NOTICE: &str = "\
✨ Update available!
0.140.0 -> 0.142.0

> 1. Update now
  2. Not now
  3. Skip until next version

Press enter to continue";

#[test]
fn codexs_interactive_update_prompt_is_always_skipped() {
    let dialog = super::dialog::blocking_dialog(CODEX_UPDATE_NOTICE)
        .expect("codex's interactive update prompt must be recognised");
    assert!(dialog.what.contains("update available"), "{dialog:?}");
    // The worker always skips it. The safety is in the *order*: every key before
    // the final confirm must be a Down-arrow (which only moves the cursor, never
    // confirms), so no keystroke can land on the dangerous "Update now" default,
    // and the closing Enter falls on "Skip until next version".
    let keys = dialog
        .dismissal
        .expect("the update prompt must carry a skip dismissal");
    assert_eq!(
        keys.last(),
        Some(&b"\r".as_slice()),
        "the sequence must end by confirming the Skip option: {keys:?}"
    );
    assert!(
        keys.len() >= 2 && keys[..keys.len() - 1].iter().all(|k| k == b"\x1b[B"),
        "every key before the confirm must be a cursor-only Down-arrow, never one \
         that could run the update: {keys:?}"
    );
}

#[test]
fn the_passive_update_banner_is_not_treated_as_a_dialog() {
    // On codex 0.142.5 the update notice is a passive banner with no numbered
    // choices — it does not block the composer, so it must NOT be recognised as a
    // dialog (the marker is the interactive Skip option, absent from the banner).
    let banner = "✨  Update available! 0.142.5 -> 0.145.0\n\
                  Run npm install -g @openai/codex to update.\n\
                  See full release notes: https://github.com/openai/codex/releases/latest";
    assert_eq!(super::dialog::blocking_dialog(banner), None);
}

#[test]
fn prose_about_updates_is_not_mistaken_for_the_notice() {
    // The marker is the distinctive Skip option, not the word "update", so an
    // agent discussing releases must not be dismissed as a modal.
    for screen in [
        "An update is available for the dependency; I'll bump it.",
        "> update the changelog and skip the release notes for now",
    ] {
        assert_eq!(
            super::dialog::blocking_dialog(screen),
            None,
            "false positive on: {screen:?}"
        );
    }
}
