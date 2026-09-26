//! Shared fixture types for daemon runtime tests.

#[allow(unused_imports)]
use super::*;

/// Recorded `(recipient, body)` pairs captured by [`recording_send`].
pub(in super::super) type Recorded = Arc<StdMutex<Vec<(String, String)>>>;
