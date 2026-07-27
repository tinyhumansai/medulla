//! Sender side: turn a pair of screens into the smallest frame that carries the
//! newer one, and coalesce raw cells into the runs a frame is made of.
//!
//! Everything here is pure. The sampler that decides *when* to call it, and the
//! transport that carries the result, live in the app crate — which is what lets
//! the whole diff be tested against literal grids with no pty and no network.

use super::types::{FrameDecision, RowUpdate, RunStyle, ScreenFrame, ScreenGrid, ScreenRun};

/// Merge a row of styled cells into runs, dropping trailing unstyled blanks.
///
/// A 120-column row is typically two or three runs; emitting one per cell would
/// inflate every frame for no visual difference. Trailing blanks are trimmed
/// only when they are *unstyled* — a harness status bar is a run of styled
/// spaces, and trimming that would erase the bar. This mirrors the coalescing
/// the worker's own renderer already does when painting to ratatui.
pub fn coalesce_runs<I>(cells: I) -> Vec<ScreenRun>
where
    I: IntoIterator<Item = (String, RunStyle)>,
{
    let mut runs: Vec<ScreenRun> = Vec::new();
    for (text, style) in cells {
        match runs.last_mut() {
            Some(run) if run.style == style => run.text.push_str(&text),
            _ => runs.push(ScreenRun::new(text, style)),
        }
    }
    // Only the final run can be trailing blanks, since any styled run after it
    // would have kept it from being last.
    if let Some(last) = runs.last_mut() {
        if last.style == RunStyle::default() {
            let trimmed = last.text.trim_end();
            if trimmed.is_empty() {
                runs.pop();
            } else if trimmed.len() != last.text.len() {
                last.text.truncate(trimmed.len());
            }
        }
    }
    runs
}

/// The rows in which `next` differs from `previous`.
///
/// Returns `None` when a diff is not meaningful — the grids are different sizes,
/// so row indices do not refer to the same rows in both. Callers must send a
/// full frame in that case; [`build_frame`] does.
pub fn changed_rows(previous: &ScreenGrid, next: &ScreenGrid) -> Option<Vec<RowUpdate>> {
    if !previous.same_size(next) {
        return None;
    }
    let mut changed = Vec::new();
    for (y, runs) in next.lines.iter().enumerate() {
        // A short `previous.lines` reads as "this row is new", which is the
        // conservative answer for a malformed grid.
        if previous.lines.get(y) != Some(runs) {
            changed.push(RowUpdate {
                y: y as u16,
                runs: runs.clone(),
            });
        }
    }
    Some(changed)
}

/// Every row of `grid`, as a full-frame row list.
fn all_rows(grid: &ScreenGrid) -> Vec<RowUpdate> {
    grid.lines
        .iter()
        .enumerate()
        .map(|(y, runs)| RowUpdate {
            y: y as u16,
            runs: runs.clone(),
        })
        .collect()
}

/// Build the frame that carries `next` to a viewer holding `previous`.
///
/// `previous` is the last screen the viewer is known to hold and `base_seq` its
/// sequence number; pass `None` on the first frame of a stream or whenever the
/// viewer has asked to resynchronise. A full frame is produced when there is no
/// previous screen or when the geometry changed — a delta across a resize is not
/// merely stale, it addresses rows that no longer exist.
///
/// Returns [`FrameDecision::Unchanged`] when nothing at all moved, including the
/// cursor, so the caller can skip the send entirely.
pub fn build_frame(
    previous: Option<&ScreenGrid>,
    next: &ScreenGrid,
    task_id: &str,
    seq: i64,
    base_seq: i64,
) -> FrameDecision {
    let delta =
        previous.and_then(|previous| changed_rows(previous, next).map(|rows| (previous, rows)));

    let (full, rows_changed) = match delta {
        Some((previous, rows)) => {
            // Nothing moved at all — not the content, not the cursor. The
            // sampler sends nothing rather than a frame that would apply as a
            // no-op.
            if rows.is_empty()
                && previous.cursor == next.cursor
                && previous.hide_cursor == next.hide_cursor
            {
                return FrameDecision::Unchanged;
            }
            (false, rows)
        }
        None => (true, all_rows(next)),
    };

    FrameDecision::Send(ScreenFrame {
        task_id: task_id.to_string(),
        seq,
        base_seq,
        full,
        cols: next.cols,
        rows: next.rows,
        cursor: next.cursor,
        hide_cursor: next.hide_cursor,
        rows_changed,
    })
}
