//! Per-session CPU, memory, and disk-I/O sampling for harness pane titles.
//!
//! Where the sibling process monitor answers "what is Medulla costing this
//! machine?" and [`device`](super::device) answers "how much room is left?",
//! this module answers the question an operator actually has in front of a
//! pane: *what is this agent costing me right now?* — summed over the harness's
//! whole process tree, because the compiler it forked is the part that costs.
//!
//! Sampling is throttled far below the render cadence, and shared across every
//! session: the host refresh a reading needs enumerates the machine's processes
//! once, and each session's tree is then walked out of that one snapshot.

mod format;
mod monitor;
mod types;

#[cfg(test)]
mod tests;

pub use format::{session_segments, session_usage};
pub use types::{SessionMonitor, SessionSnapshot};
