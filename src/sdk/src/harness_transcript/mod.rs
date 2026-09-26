//! What a harness actually *said* while it served one task, kept for replay.
//!
//! A workflow `agent` node runs headless: no pane, no pty, nobody watching. All
//! that survived it was the final reply, so the run view could say a step
//! succeeded and took four minutes without being able to say what happened in
//! them. When a node does something surprising — the wrong file, a tool that
//! kept failing, a plan abandoned halfway — the reply is the one part of the
//! turn that does not show it.
//!
//! This module is the durable middle ground between the two things that already
//! exist and neither of which answers that question:
//!
//! * [`crate::harness_work`] folds the same event stream into a *snapshot* —
//!   the todo list, the files touched, the PR opened. It says where the turn got
//!   to, deliberately discarding the order it got there in.
//! * The status frames the daemon emits are live progress. Nothing keeps them.
//!
//! A transcript is the ordered account: each message, each tool call, each
//! error, in the sequence the harness produced them.
//!
//! # Bounded on the way in, not on the way out
//!
//! A single node can emit tens of thousands of events, and a run record is read
//! in full by every surface that shows it. So the collector caps both the entry
//! count ([`MAX_ENTRIES`]) and the bytes ([`MAX_TEXT_BYTES`]) as events arrive,
//! rather than recording everything and trimming at the end — the point of the
//! cap is that the memory is never held, not merely that the file is small.
//!
//! Overflow is *reported*, never silent: the collector appends one final entry
//! saying how many events it dropped. A transcript that quietly stops halfway
//! reads as a harness that stopped halfway, which is the one wrong conclusion
//! this could lead an operator to.
//!
//! # Layout
//!
//! [`types`] holds the durable entry and the collector; the fold from
//! [`HarnessSemanticEvent`](crate::daemon::mappers::HarnessSemanticEvent) into
//! an entry lives in [`fold`], so the wire vocabulary is translated in exactly
//! one place.

mod fold;
mod types;

#[cfg(test)]
mod tests;

pub use types::{TranscriptCollector, TranscriptEntry, MAX_ENTRIES, MAX_TEXT_BYTES};
