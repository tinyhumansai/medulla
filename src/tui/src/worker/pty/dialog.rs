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

/// A startup dialog that has to be answered before the harness will take work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockingDialog {
    /// What the harness is waiting for, in the operator's terms.
    pub what: &'static str,
    /// What to do about it.
    pub remedy: &'static str,
}

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
