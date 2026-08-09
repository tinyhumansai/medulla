//! Recognising a harness that is waiting on the *operator* rather than on the
//! model.
//!
//! A harness in a pane is a black box with one bit of state the operator
//! actually needs: is it working, or is it stuck waiting for me. Claude Code
//! stops on a permission prompt, Codex stops on an approval, OpenCode stops on
//! its permission dialog — and until it is answered nothing happens. In a fleet
//! of panes, only one of which is on screen at a time, that state is invisible:
//! the harness is not dead, not idle, and not producing output, so every other
//! signal the rail has (liveness, `busy`, exit status) reads exactly the same as
//! a harness that is thinking hard.
//!
//! So this module answers "does this session want me" from the one surface that
//! always tells the truth — the screen the harness painted — and the rail turns
//! that into a blinking row.
//!
//! Three signals, most specific first:
//!
//! 1. **Startup dialogs** already recognised by [`super::dialog`]. Trust, the
//!    bypass disclaimer, codex's update prompt. Shared verbatim rather than
//!    re-listed here so the two cannot drift, and so the wording the operator
//!    reads is the same wording the injector reports.
//! 2. **Per-harness prompt markers** — the distinctive phrases each CLI puts on
//!    screen when it is asking. Matched on squashed text, so a match does not
//!    depend on where a full-screen TUI chose to put its spaces or how it wrapped
//!    the line.
//! 3. **Structure**, for everything unrecognised: a numbered menu with a
//!    selection caret resting on an option, or a `(y/n)` confirmation. Every one
//!    of these harnesses draws its questions that way, so this keeps working
//!    when a CLI rewords a prompt — which they do, often, and silently.
//!
//! 4. **Blocking errors** — a usage limit, a dead credential, an API failure the
//!    harness will not retry past. Printed *instead of* a completed turn, so the
//!    work did not happen and a person has to intervene. Matched
//!    provider-agnostically, because the wording comes from the model API rather
//!    than from any one CLI's chrome.
//!
//! A fifth signal lives in the manager rather than here, because it is not on
//! the screen: the **terminal bell**. It is the vaguest cue and the most
//! universal one, so it is kept as the fallback that loses to anything named
//! ([`AttentionKind`] is ordered for exactly that).
//!
//! Two more come from the session's *life* rather than its screen, and live in
//! [`session`]: the harness **died**, and a dispatched turn **finished** and is
//! being held for someone to read. Neither can be seen in a terminal — a dead
//! harness's screen is frozen on whatever it last painted, and a settled one
//! looks exactly like a session nobody has used — so a row that consulted only
//! the screen went quiet at precisely the two moments it should not have.
//!
//! Matching screen text is a heuristic and is treated as one. A false negative
//! costs an operator the blink they would have got; a false positive costs a row
//! that says "needs you" about a harness that is merely thinking. Neither
//! changes what is *sent* to the harness — nothing here writes to a pty — so the
//! blast radius of a wrong guess is one row of chrome. That is what lets the
//! structural fallback be as liberal as it is.

mod detect;
mod hook;
mod session;
mod types;

#[cfg(test)]
mod tests;

pub use detect::{bell_cue, detect, is_working};
pub use hook::hook_attention;
pub use session::{lifecycle_cue, row_cue};
pub use types::{AttentionKind, HarnessAttention};

/// The glyph a waiting harness is marked with, in both TUIs.
///
/// Deliberately not one of the lifecycle glyphs (`●`, `✓`, `✕`): those answer
/// "how is it", and this answers "it needs you", which is a different question
/// and the only one that warrants taking a row over. Defined here so the
/// orchestrator's rail and the worker's session list cannot drift into marking
/// the same state with two different symbols.
pub const ATTENTION_GLYPH: char = '⚠';
