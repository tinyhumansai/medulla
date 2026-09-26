//! The handoff brief: what an operator hands back along with a harness.
//!
//! Handing a harness back used to be the *absence of a veto* — one process-local
//! flag flipped, and the orchestrator was told nothing. It found out only if a
//! task frame happened to arrive for the same conversation later, and it arrived
//! with no idea what the person had been doing. This module is the other half:
//! the brief that turns a handback into a transfer of *work*.
//!
//! Everything here is pure. The brief travels on the roster advert
//! (`medulla:register_agents`), which the hub already re-emits on every roster
//! mutation — so a control change is already an event and needs no plane of its
//! own. Keeping the payload buildable without a socket is what makes the whole
//! contract testable; the emit itself is a one-line wrapper in
//! [`super::handle`].
//!
//! ## Bounds
//!
//! The advert is re-emitted often, and the brief rides on it, so it is clamped
//! hard. A brief is meant to orient a reader, not to archive a session: the
//! harness's own transcript is where deep history lives.

mod types;

#[cfg(test)]
mod tests;

pub use types::{HandoffControl, HarnessHandoff};

/// Longest operator note carried, in characters.
pub const NOTE_MAX: usize = 2_000;

/// Screen lines carried in the transcript excerpt.
pub const TRANSCRIPT_LINES: usize = 80;

/// Longest single transcript line, in characters.
///
/// A harness pane is often much wider than it is readable, and a brief full of
/// box-drawing rules teaches a reader nothing.
pub const LINE_MAX: usize = 400;

/// Ceiling on the joined transcript, in characters.
///
/// Belt and braces over the per-line and line-count limits: a pane of 80 long
/// lines would otherwise be 32 KB on an advert that re-emits whenever a worker
/// is added.
pub const TRANSCRIPT_MAX: usize = 8_000;

/// Clamp a brief to what the advert will carry.
///
/// Trims the note and drops it when it is blank, trims and truncates each line,
/// drops trailing blank lines, keeps the last [`TRANSCRIPT_LINES`], and caps the
/// joined result at [`TRANSCRIPT_MAX`] — recording in `transcript_truncated`
/// whether any of that discarded anything.
///
/// Truncation keeps the *end* throughout. The last thing on the screen is what
/// the operator was looking at when they handed it over, and it is the part that
/// says where the work stopped.
pub fn normalize(mut brief: HarnessHandoff, lines: &[String]) -> HarnessHandoff {
    brief.note = brief
        .note
        .map(|note| note.trim().to_string())
        .filter(|note| !note.is_empty())
        .map(|note| truncate_chars(&note, NOTE_MAX).0);

    let mut truncated = false;
    let mut kept: Vec<String> = lines
        .iter()
        .map(|line| {
            let (line, cut) = truncate_chars(line.trim_end(), LINE_MAX);
            truncated |= cut;
            line
        })
        .collect();
    while kept.last().is_some_and(|line| line.is_empty()) {
        kept.pop();
    }
    if kept.len() > TRANSCRIPT_LINES {
        kept.drain(..kept.len() - TRANSCRIPT_LINES);
        truncated = true;
    }

    let joined = kept.join("\n");
    // Trim from the front: the tail is the part worth keeping.
    let transcript = if joined.chars().count() > TRANSCRIPT_MAX {
        truncated = true;
        let skip = joined.chars().count() - TRANSCRIPT_MAX;
        joined.chars().skip(skip).collect()
    } else {
        joined
    };

    brief.transcript = transcript;
    brief.transcript_truncated = truncated;
    brief
}

/// Truncate to `max` characters, reporting whether anything was dropped.
///
/// Counts characters rather than bytes so a multi-byte glyph is never split into
/// invalid UTF-8 — terminal output is full of box-drawing and emoji.
fn truncate_chars(text: &str, max: usize) -> (String, bool) {
    if text.chars().count() <= max {
        return (text.to_string(), false);
    }
    (text.chars().take(max).collect(), true)
}
