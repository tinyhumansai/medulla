//! Rendering one session's reading into the compact form a pane title holds.
//!
//! Kept apart from sampling so the wording is a pure function of an injected
//! [`SessionSnapshot`] and can be asserted exactly.

use super::types::SessionSnapshot;

/// The session's cost as title segments: CPU, then memory, then disk I/O.
///
/// Always all three, in that order, whatever the numbers are. A title whose
/// segments appear and vanish with activity is one an operator has to re-read
/// every frame to find the number they were watching — and "this session is
/// doing no I/O right now" is itself worth showing, since it is what separates
/// a harness that is thinking from one that is stuck.
///
/// Deliberately not gated on the `[appearance]` resource switches: those govern
/// the status line's *Medulla process* readings, which default to off, and the
/// session title is a different question asked about a different subject.
pub fn session_segments(sample: SessionSnapshot) -> Vec<String> {
    vec![
        format!("CPU {:.0}%", sample.cpu_fraction * 100.0),
        format!("RAM {}", super::super::bytes(sample.memory_bytes as f64)),
        format!("IO {}/s", super::super::bytes(sample.disk_bytes_per_second)),
    ]
}

/// The same reading as one title-ready string.
pub fn session_usage(sample: SessionSnapshot) -> String {
    session_segments(sample).join(" · ")
}
