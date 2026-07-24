//! Delivering a prompt into a live harness session — the typing half of
//! [`super::launch`]'s encoding.
//!
//! Writing bytes at a pty is easy; getting an interactive TUI to *accept* them
//! is not, and the difference is timing and terminal modes. This module owns
//! both, so the executor can stay a description of a turn rather than a
//! collection of sleeps.
//!
//! Three things have to be true before a prompt actually runs:
//!
//! 1. **The harness is listening.** A child spawned microseconds ago has not
//!    installed its input handling yet; bytes written into that window are
//!    simply gone. Waiting is not optional on a cold start.
//! 2. **The paste is encoded the way the child asked for.** Bracketed-paste
//!    markers belong only to an application that enabled the mode. Sent to one
//!    that did not, `ESC[200~` arrives as an escape keypress followed by
//!    literal text — which mangles the prompt and can dismiss the composer
//!    outright.
//! 3. **The paste actually reached the composer.** This is checked, not timed.
//!    Claude Code sets its terminal modes and paints a first screen seconds
//!    before it will accept input — it is still loading MCP servers, and goes
//!    quiet while it does — so every timing heuristic has a window where the
//!    paste is silently discarded and Enter submits an empty composer. The
//!    prompt is looked for on screen and re-sent until it is there.
//!
//! Each wait is on an observed condition rather than a fixed sleep, so a cold
//! machine is tolerated and a warm one is not penalised.

use std::time::Duration;

use super::dialog::{blocking_dialog, BlockingDialog};
use super::launch::{bracket_paste, submit_sequence};
use super::manager::PtyManager;

/// How long to wait for a freshly-spawned harness to come up.
///
/// Generous because it is only ever paid once per session, and paying it is
/// strictly better than typing into a process that is not listening yet.
const READY_BUDGET: Duration = Duration::from_secs(10);

/// Floor on how soon a session is considered ready.
///
/// Measured against a real cold start: Claude Code sets bracketed-paste mode at
/// ~0.3s and paints its first screen after that, so anything shorter decides
/// what is on screen before there is a screen. Paid once per session.
const MIN_GRACE: Duration = Duration::from_millis(750);

/// How long to wait for a paste to show up on screen before re-sending it.
const ECHO_BUDGET: Duration = Duration::from_millis(2_000);

/// How many times to re-send a paste the harness dropped.
///
/// Bounded because the failure mode of over-retrying is a composer holding the
/// same prompt several times over, which is worse than reporting that it never
/// took: a duplicated prompt runs, and runs wrong.
const PASTE_ATTEMPTS: u32 = 3;

/// Output-free time that counts as "finished rendering".
const QUIET_MS: i64 = 150;

/// How much of the prompt to look for on screen to call a paste landed.
///
/// A prefix rather than the whole thing: the pane wraps, and a long prompt is
/// re-flowed or replaced by a placeholder. Long enough not to collide with
/// chrome, short enough to survive the harness's own formatting.
const NEEDLE_CHARS: usize = 24;

/// How often the readiness conditions are re-checked.
const TICK: Duration = Duration::from_millis(25);

/// How many stacked startup dialogs to clear before giving up.
///
/// A launch can raise more than one modal in sequence (an update notice, then a
/// trust prompt); a small bound clears a short stack while still stopping us
/// looping forever on a dialog our keystrokes are not, in fact, dismissing.
const MAX_DIALOGS: u32 = 3;

/// Pause after each dismissal keystroke, so an arrow move lands and the cursor
/// settles before the next key — the difference between selecting the Skip row
/// and confirming whatever was highlighted first.
const KEY_PACING: Duration = Duration::from_millis(120);

/// How long to wait for a dismissed dialog to leave the screen before deciding
/// the keystrokes did not clear it.
const DIALOG_CLEAR_BUDGET: Duration = Duration::from_millis(3_000);

/// Type `text` into a session and submit it, as a human at that terminal would.
///
/// Waits for the harness to be ready, encodes the paste the way the child asked
/// for, waits for it to be taken, then presses Enter. Errors if the session is
/// unknown or is no longer running.
pub async fn inject_prompt(sessions: &PtyManager, id: &str, text: &str) -> Result<(), String> {
    let bracketed = await_ready(sessions, id).await?;

    // Modes set and screen painted does not mean "ready for a prompt". A
    // harness sitting on a startup dialog discards the paste and reads the
    // Return as an answer to whatever it is asking. A dialog we know how to
    // answer (codex's update notice) is dismissed here; one we can only name
    // (claude's trust / bypass modals, which preflight should have cleared) is
    // reported — never typed a prompt into.
    clear_startup_dialogs(sessions, id).await?;

    if !bracketed {
        // No composer to commit to: a line-oriented reader takes the bytes and
        // the return in one go, and there is no rendering to wait on.
        sessions.write(id, text.as_bytes())?;
        return sessions.write(id, submit_sequence());
    }

    // The child has a composer, so the paste must be *in* it before Enter can
    // mean "send this" — and whether it arrived is something to check rather
    // than time. Claude Code sets its terminal modes and paints a first screen
    // seconds before it will accept input (it is still loading MCP servers, and
    // goes quiet while it does), so every timing heuristic tried here had a
    // window where the paste was silently discarded and Enter submitted an
    // empty composer.
    let needle = needle_of(text);
    let before = occurrences(&screen_text(sessions, id), &needle);
    let mut landed = false;
    for _ in 0..PASTE_ATTEMPTS {
        sessions.write(id, &bracket_paste(text))?;
        if await_paste(sessions, id, &needle, before).await {
            landed = true;
            break;
        }
    }
    if !landed {
        // Refuse rather than press Enter hopefully: a bare Return on an empty
        // composer does nothing, and the turn then dies thirty seconds later
        // against a transcript that was never going to exist.
        return Err(format!(
            "{id}: the harness never took the prompt (tried {PASTE_ATTEMPTS} times)"
        ));
    }

    // Let the composer settle before the Return, so it is not swallowed by the
    // paste block still being committed.
    await_still(sessions, id).await;
    sessions.write(id, submit_sequence())
}

/// Clear any startup dialog standing between the session and its composer.
///
/// A dialog the worker knows how to answer (codex's update notice) is dismissed
/// with its safe keystroke sequence and we move on; one it can only recognise
/// (claude's trust / bypass disclaimer, which the startup preflight should have
/// cleared) is reported as an error, because typing a prompt into it would paste
/// into a live modal and read the Return as an answer to whatever it asks.
///
/// Loops so a short stack of dialogs is cleared in turn, and gives up — with the
/// dialog named — if one it thought it could dismiss is still on screen after
/// [`MAX_DIALOGS`] attempts.
async fn clear_startup_dialogs(sessions: &PtyManager, id: &str) -> Result<(), String> {
    for _ in 0..MAX_DIALOGS {
        let Some(dialog) = blocking_dialog(&screen_text(sessions, id)) else {
            return Ok(());
        };
        let Some(keys) = dialog.dismissal else {
            return Err(format!("{} — {}", dialog.what, dialog.remedy));
        };
        for chunk in keys {
            sessions.write(id, chunk)?;
            tokio::time::sleep(KEY_PACING).await;
        }
        await_dialog_cleared(sessions, id, dialog).await;
    }
    // Still on a dialog after the bounded attempts: the keystrokes did not clear
    // it, so name it rather than type a prompt into a modal that is still up.
    match blocking_dialog(&screen_text(sessions, id)) {
        Some(dialog) => Err(format!(
            "{} — {} (dismissal did not clear it)",
            dialog.what, dialog.remedy
        )),
        None => Ok(()),
    }
}

/// Wait for `dialog` to leave the screen after its dismissal keys were sent.
///
/// Returns once the recognised dialog is no longer the one on screen — either it
/// is gone, or a different dialog has taken its place, which the caller's loop
/// then handles in the next pass. Bounded by [`DIALOG_CLEAR_BUDGET`].
async fn await_dialog_cleared(sessions: &PtyManager, id: &str, dialog: &BlockingDialog) {
    let started = tokio::time::Instant::now();
    while started.elapsed() < DIALOG_CLEAR_BUDGET {
        if blocking_dialog(&screen_text(sessions, id)) != Some(dialog) {
            return;
        }
        tokio::time::sleep(TICK).await;
    }
}

/// The screen fragment that shows a prompt reached the composer.
///
/// Whitespace-free so it survives the pane wrapping mid-prompt.
fn needle_of(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .take(NEEDLE_CHARS)
        .collect()
}

/// How many times `needle` appears in `screen`.
///
/// Counted rather than tested, because an unbound session's screen still holds
/// the peer's previous turns: a prompt that repeats a phrase would otherwise
/// look like it had already landed before it was sent.
fn occurrences(screen: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let squashed: String = screen
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    squashed.matches(needle).count()
}

/// Wait for the pasted prompt to appear on screen.
///
/// True once the needle appears more often than it did before the paste, or the
/// harness shows its own paste placeholder — Claude Code collapses a long or
/// multi-line paste to `[Pasted text #1 +N lines]` rather than echoing it, so
/// looking only for the literal text would call a landed paste lost and send it
/// again.
async fn await_paste(sessions: &PtyManager, id: &str, needle: &str, before: usize) -> bool {
    let started = tokio::time::Instant::now();
    while started.elapsed() < ECHO_BUDGET {
        let screen = screen_text(sessions, id);
        if occurrences(&screen, needle) > before || occurrences(&screen, "pastedtext") > 0 {
            return true;
        }
        tokio::time::sleep(TICK).await;
    }
    false
}

/// Wait for the screen to stop changing, bounded by [`ECHO_BUDGET`].
async fn await_still(sessions: &PtyManager, id: &str) {
    let started = tokio::time::Instant::now();
    while started.elapsed() < ECHO_BUDGET {
        let Some(row) = sessions.row(id) else { return };
        if row.idle_ms(medulla::clock::now_millis()) >= QUIET_MS {
            return;
        }
        tokio::time::sleep(TICK).await;
    }
}

/// A session's rendered screen as plain text, for recognising what is on it.
fn screen_text(sessions: &PtyManager, id: &str) -> String {
    let Some(snapshot) = sessions.screen_rows(id) else {
        return String::new();
    };
    snapshot
        .cells
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| cell.text.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Wait until the session can be typed at, returning whether to bracket the
/// paste.
///
/// Ready means the harness has stopped painting, not merely that it has set its
/// terminal modes. Those are very different moments: Claude Code enables
/// bracketed-paste mode about 0.3s into a cold start, while its first screen —
/// which may be a modal asking a question — lands later. Treating the mode bit
/// as readiness meant deciding what was on screen before anything was, so a
/// dialog went unrecognised and the prompt was typed into it.
///
/// Hence both conditions, for every harness: a floor of [`MIN_GRACE`] and then
/// [`QUIET_MS`] with no output. Gives up at [`READY_BUDGET`] and lets the caller
/// try anyway — a prompt typed at a slow harness may still land, whereas
/// refusing guarantees it does not.
async fn await_ready(sessions: &PtyManager, id: &str) -> Result<bool, String> {
    let started = tokio::time::Instant::now();
    let mut bracketed = false;
    loop {
        let row = sessions
            .row(id)
            .ok_or_else(|| format!("{id}: no such session"))?;
        if !row.state.is_running() {
            return Err(format!("{id}: session is not running"));
        }
        // Latched: the mode says how to encode the paste, and a harness that
        // asked for it once has not changed its mind by the time we type.
        bracketed |= sessions.bracketed_paste(id) == Some(true);

        let elapsed = started.elapsed();
        if elapsed >= READY_BUDGET {
            return Ok(bracketed);
        }
        if elapsed >= MIN_GRACE && row.idle_ms(medulla::clock::now_millis()) >= QUIET_MS {
            return Ok(bracketed);
        }
        tokio::time::sleep(TICK).await;
    }
}
