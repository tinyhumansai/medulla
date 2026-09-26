//! Data types for the `turn_stream` module.
#[allow(unused_imports)]
use super::*;
/// The outcome of folding one raw line.
#[derive(Debug, Clone)]
pub struct LineFold {
    /// Semantic events this line produced, in order. Drive status frames from
    /// these; they are identical in both modes.
    pub events: Vec<HarnessSemanticEvent>,
    /// The turn's answer, present only on the line that ended it.
    pub reply: Option<String>,
}
/// Folds a harness's raw output into semantic events and a completion.
///
/// One per turn. Feed it every line from whichever source the mode provides.
pub struct TurnStream {
    pub(super) mapper: HarnessLineMapper,
    pub(super) watcher: TurnWatcher,
    pub(super) line_no: i64,
    pub(super) events_seen: usize,
}
