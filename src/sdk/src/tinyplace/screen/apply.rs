//! Viewer side: fold an incoming frame into the screen held for a session.
//!
//! The counterpart to [`diff`](super::diff), and equally pure — the hub's store
//! and renderer sit above this. Every rejection path returns
//! [`ApplyOutcome::NeedsResync`] rather than a partial write, so a view is
//! always either current or visibly stale; it is never quietly wrong.

use super::types::{ApplyOutcome, ScreenFrame, ScreenGrid, ScreenView};

/// Rebuild a whole grid from a full frame.
fn grid_from_full(frame: &ScreenFrame) -> ScreenGrid {
    let mut lines = vec![Vec::new(); frame.rows as usize];
    for row in &frame.rows_changed {
        if let Some(slot) = lines.get_mut(row.y as usize) {
            *slot = row.runs.clone();
        }
    }
    ScreenGrid {
        cols: frame.cols,
        rows: frame.rows,
        lines,
        cursor: frame.cursor,
        hide_cursor: frame.hide_cursor,
    }
}

/// Fold `frame` into `view`, replacing or patching it in place.
///
/// A full frame always applies and resets the view. A delta applies only when
/// the view holds exactly `frame.base_seq` at the frame's geometry; anything
/// else — no view yet, a sequence gap, a resize, or a row index outside the
/// grid — leaves the view untouched and asks for a resynchronise.
///
/// The sequence check is what makes the mailbox's lack of ordering guarantees
/// survivable: a dropped, duplicated, or reordered delta all fail the same way
/// and recover through the same full frame.
pub fn apply_frame(view: &mut Option<ScreenView>, frame: &ScreenFrame) -> ApplyOutcome {
    if frame.full {
        *view = Some(ScreenView {
            grid: grid_from_full(frame),
            seq: frame.seq,
        });
        return ApplyOutcome::Applied;
    }

    let Some(current) = view.as_mut() else {
        return ApplyOutcome::NeedsResync;
    };
    if current.seq != frame.base_seq {
        return ApplyOutcome::NeedsResync;
    }
    // A sender that follows the protocol never sends a delta across a resize.
    // One that does not is not to be trusted with row indices.
    if current.grid.cols != frame.cols || current.grid.rows != frame.rows {
        return ApplyOutcome::NeedsResync;
    }
    // Validate before mutating: a frame naming a row outside the grid is
    // malformed, and half-applying it would leave a view that looks current
    // while showing something that was never on screen.
    if frame
        .rows_changed
        .iter()
        .any(|row| row.y as usize >= current.grid.lines.len())
    {
        return ApplyOutcome::NeedsResync;
    }

    for row in &frame.rows_changed {
        current.grid.lines[row.y as usize] = row.runs.clone();
    }
    current.grid.cursor = frame.cursor;
    current.grid.hide_cursor = frame.hide_cursor;
    current.seq = frame.seq;
    ApplyOutcome::Applied
}
