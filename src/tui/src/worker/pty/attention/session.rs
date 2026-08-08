//! Attention cues that come from the session's *life* rather than its screen.
//!
//! [`detect`](super::detect) reads the terminal, which is the right surface for
//! everything a harness says. It is the wrong surface for the two things a
//! harness cannot say, because by then it is not talking:
//!
//! - the child **died**, or a write to it failed — the screen is frozen on
//!   whatever it last painted, which may well be a perfectly ordinary composer;
//! - a dispatched turn **settled** and the session is being held open for the
//!   operator to read, which the screen renders identically to any other idle
//!   prompt.
//!
//! Both are facts the manager already holds and neither ever reached the rail,
//! so a harness that fell over stopped blinking and started looking calm. This
//! module is where the row's own state becomes a cue, so those two join the
//! screen-derived ones in a single vocabulary that one renderer can draw and one
//! counter can count.

use super::super::types::{PtyState, SessionRow};
use super::{AttentionKind, HarnessAttention};

/// The cue this row's lifecycle warrants, ignoring anything on its screen.
///
/// Returns `None` for the ordinary cases — running, or exited cleanly with
/// nothing left to read — so a caller can chain it with the screen-derived cue
/// and let precedence decide.
///
/// The row's stable last-output timestamp stamps the cue. Reaping writes that
/// timestamp when a child exits, so it is the lifecycle transition time for
/// failures; a retained session has likewise just completed a turn. Reusing it
/// prevents every render frame from resetting the displayed elapsed time.
pub fn lifecycle_cue(row: &SessionRow, _now: i64) -> Option<HarnessAttention> {
    if let Some(what) = failure_reason(row) {
        return Some(HarnessAttention::new(
            AttentionKind::Failed,
            what,
            row.last_output_at,
        ));
    }
    // Retained means a task finished here and the session was deliberately kept
    // standing. Nobody has taken it and nothing more will happen in it, which is
    // precisely the state that has no other way of announcing itself.
    if row.retained {
        return Some(HarnessAttention::new(
            AttentionKind::Completed,
            format!("{} finished — read and release", row.provider.as_str()),
            row.last_output_at,
        ));
    }
    None
}

/// Why this session is beyond help, in the operator's terms.
///
/// A recorded write error wins over the exit status because it says *what*
/// happened; an exit code alone can only say that something did. A clean exit is
/// not a failure and a running session has not had one yet.
fn failure_reason(row: &SessionRow) -> Option<String> {
    if let Some(error) = row.last_error.as_deref() {
        return Some(format!("{} failed: {error}", row.provider.as_str()));
    }
    match row.state {
        PtyState::Failed => Some(format!("{} could not run", row.provider.as_str())),
        PtyState::Exited { code: Some(code) } if code != 0 => {
            Some(format!("{} exited with {code}", row.provider.as_str()))
        }
        _ => None,
    }
}

/// The single cue a row should be drawn and counted by.
///
/// Lifecycle first, then the screen: a dead harness's last painted menu is not a
/// question anyone can answer, and a retained session's idle composer says less
/// than the fact that its task is done.
///
/// The screen half is dropped entirely once the child has gone, for the same
/// reason. A session that exited while a permission menu was up leaves that menu
/// frozen on its last frame; answering it is no longer possible, and a row that
/// keeps asking is a row the operator cannot clear.
pub fn row_cue(row: &SessionRow, now: i64) -> Option<HarnessAttention> {
    lifecycle_cue(row, now).or_else(|| {
        row.state
            .is_running()
            .then(|| row.attention.clone())
            .flatten()
    })
}
