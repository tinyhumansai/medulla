//! Recognising a harness that is waiting on a startup dialog rather than on a
//! prompt.
//!
//! A harness has its terminal modes set and its screen painted long before it
//! is ready to be *asked* something. Claude Code in particular opens with a
//! workspace trust dialog — "Is this a project you created or one you trust?" —
//! and that dialog is modal: pasted text is discarded, and the Return that
//! would have submitted a prompt instead picks whatever option the cursor is
//! resting on.
//!
//! This is not the headless path's problem, which is why it went unnoticed:
//! Claude documents the trust dialog as skipped when stdout is not a TTY. The
//! worker TUI exists precisely to give the harness a TTY, so it inherits a gate
//! the daemon never saw. `--dangerously-skip-permissions` does **not** clear it
//! (verified against the installed CLI, which shows the dialog either way).
//!
//! So a turn must never be typed blind. Recognising the dialog turns a thirty
//! second timeout and a misleading "could not find the transcript" into
//! something the operator can act on. The worker clears both of these dialogs at
//! startup (see [`crate::worker::trust`]), so one appearing here means that
//! preflight failed — and saying which dialog it is names the failure.
//!
//! Matching is on screen text and therefore a heuristic: it can only produce a
//! better-worded failure for a turn that was already failing, never a wrong
//! success. A dialog that goes unrecognised falls back to the existing timeout.
//!
//! Some dialogs the worker can do more than name — it can *answer* them. Codex
//! opens a fresh workspace on a trust prompt ("Do you trust the contents of this
//! directory?") whose default is `› 1. Yes, continue`; unlike claude's trust
//! modal it is not gated by any config the preflight pre-accepts, so the worker
//! answers it live. It carries a [`BlockingDialog::dismissal`] — a single Enter,
//! which confirms that already-highlighted default — and [`super::inject`]
//! presses it rather than reporting a wall. That the operator launched with the
//! bypass flag *is* the decision to run here, so confirming trust is not a choice
//! the worker is inventing (the same reasoning as [`crate::worker::trust`]).
//!
//! Codex's *interactive update* prompt is likewise dismissed rather than
//! reported: an unattended worker never wants to run the installer, so it always
//! **skips**. The one hazard is that this modal's default is `1. Update now`,
//! where Enter runs `npm install` — so the dismissal moves the cursor onto
//! `3. Skip until next version` with arrows first (which only move, never
//! confirm) and only then presses Enter (see `CODEX_SKIP_UPDATE`). On codex
//! 0.142.5 the update notice is in fact a passive *banner* that does not block
//! the composer, so only its interactive-modal variant is matched at all.
//!
//! Claude's modals, by contrast, stay report-only: they should already be
//! cleared, so meeting one means preflight failed and the fix is a config change,
//! not a keypress.

/// A startup dialog that has to be answered before the harness will take work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockingDialog {
    /// What the harness is waiting for, in the operator's terms.
    pub what: &'static str,
    /// What to do about it.
    pub remedy: &'static str,
    /// The keystrokes that safely dismiss this dialog, if the worker can clear
    /// it itself, as a sequence of raw byte writes (one write per key, so the
    /// injector can pace them and let each land before the next).
    ///
    /// `None` means report-only: the worker recognises the dialog but will not
    /// answer it, because the right fix is the [`remedy`](Self::remedy), not a
    /// blind keypress into someone else's modal.
    pub dismissal: Option<&'static [&'static [u8]]>,
}

/// A single Enter — confirms codex's trust dialog.
///
/// Its default is already highlighted (`› 1. Yes, continue`), so Enter selects
/// it: no arrow navigation, no risk of landing on `2. No, quit`. Verified live
/// against codex-cli 0.142.5, where Esc was found to *cancel* the dialog and exit
/// codex — which is exactly why the dismissal is Enter and never Esc.
const CODEX_TRUST_CONTINUE: &[&[u8]] = &[b"\r"];

/// Down, Down, Enter — always skips codex's interactive update prompt by landing
/// on `3. Skip until next version`.
///
/// Calibrated to codex's observed three-option layout (`1. Update now` /
/// `2. …` / `3. Skip until next version`) with the cursor starting on option 1.
/// The order is the safety: an unattended worker never wants the installer, and
/// the modal's default is the dangerous `Update now`, so the two Down-arrows move
/// the cursor onto the Skip row *before* the Enter — and arrows only move, they
/// never confirm, so no keystroke here can trigger the update until the cursor is
/// already on Skip.
const CODEX_SKIP_UPDATE: &[&[u8]] = &[b"\x1b[B", b"\x1b[B", b"\r"];

/// Screen fragments that identify a dialog, paired with how to describe it.
///
/// Stored whitespace-free and lowercase so they match regardless of how the
/// harness laid the words out — a full-screen TUI positions text with cursor
/// moves, and where it chose to put its spaces is not something to depend on.
const DIALOGS: &[(&[&str], BlockingDialog)] = &[
    (
        &["itrustthisfolder", "isthisaprojectyoucreated"],
        BlockingDialog {
            what: "claude is waiting for you to approve this workspace",
            remedy: "the worker normally trusts its workspace at startup, so this \
                     means that failed — check the log, or run claude in the \
                     workspace once yourself and accept the prompt",
            dismissal: None,
        },
    ),
    (
        // `--dangerously-skip-permissions` opens this, and its default option is
        // "1. No, exit" — so of all the dialogs, this is the one where typing
        // blind is worst: the Return does not mistype a prompt, it kills the
        // session. The worker normally accepts it up front; recognising it here
        // covers the case where writing the settings failed.
        &[
            "runninginbypasspermissionsmode",
            "youacceptallresponsibility",
        ],
        BlockingDialog {
            what: "claude is waiting for you to accept the bypass-permissions disclaimer",
            remedy: "the worker normally accepts this at startup, so this means \
                     that failed — check the log, or run claude with \
                     --dangerously-skip-permissions once yourself and accept it",
            dismissal: None,
        },
    ),
    (
        // Codex's trust prompt, shown on a fresh workspace even under
        // `--dangerously-bypass-approvals-and-sandbox`. `trustthecontentsofthis`
        // keys on its distinctive question and will not trip on claude's own
        // trust wording. The default (`Yes, continue`) is highlighted, so the
        // worker confirms it with a single Enter (see `CODEX_TRUST_CONTINUE`).
        &["trustthecontentsofthis"],
        BlockingDialog {
            what: "codex is asking whether to trust this workspace",
            remedy: "the worker confirms trust with Enter; if that fails, run \
                     codex in the workspace once yourself and accept the prompt",
            dismissal: Some(CODEX_TRUST_CONTINUE),
        },
    ),
    (
        // Codex's *interactive* update prompt (`1. Update now` … `3. Skip until
        // next version`). `skipuntilnextversion` is the marker: it is the Skip
        // option and appears only on this modal, never on the passive "Update
        // available!" banner (which does not block the composer, so must not be
        // treated as a dialog). The worker always skips it (see
        // `CODEX_SKIP_UPDATE`); the remedy is only the fallback message shown if
        // the skip keystrokes somehow fail to clear it.
        &["skipuntilnextversion"],
        BlockingDialog {
            what: "codex is showing its interactive \"update available\" prompt",
            remedy: "the worker skips it by choosing \"skip until next version\"; \
                     if it persists, update codex (npm install -g @openai/codex) \
                     or run it once and skip the notice yourself",
            dismissal: Some(CODEX_SKIP_UPDATE),
        },
    ),
];

/// Strip whitespace and case so a match does not depend on screen layout.
fn squash(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The dialog a session is sitting on, if it is sitting on one we recognise.
///
/// `screen` is the session's rendered screen as text.
pub fn blocking_dialog(screen: &str) -> Option<&'static BlockingDialog> {
    let squashed = squash(screen);
    DIALOGS.iter().find_map(|(markers, dialog)| {
        markers
            .iter()
            .any(|marker| squashed.contains(marker))
            .then_some(dialog)
    })
}
