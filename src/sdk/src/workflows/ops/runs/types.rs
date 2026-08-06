//! Data types that configure workflow-run operations.

use std::time::Duration;

/// How long a caller is willing to hold a `workflow_run` call open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Wait {
    /// Return as soon as the run is admitted, with its id.
    #[default]
    No,
    /// Block until the run settles, however long that takes.
    Forever,
    /// Block until the run settles or this budget expires, whichever is first.
    Until(Duration),
}

impl Wait {
    /// Whether this mode holds the call open at all.
    pub fn blocks(self) -> bool {
        !matches!(self, Self::No)
    }
}
