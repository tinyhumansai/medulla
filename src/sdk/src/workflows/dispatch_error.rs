//! Turning a hub dispatch failure into a workflow failure.
//!
//! The one piece of [`WorkflowError`] that could not move to the engine crate
//! with the rest of the stored model: [`crate::hub::RunError`] is Medulla's, and
//! the engine has never heard of a harness fleet. It stays here as a single
//! `From` impl, which the orphan rule permits because the type being converted
//! *from* is local to this crate.

use tinyflows::store::WorkflowError;

use crate::hub::RunError;

impl From<RunError> for WorkflowError {
    /// Preserve the shape of a dispatch failure rather than flattening it.
    ///
    /// The hub already distinguishes these; collapsing them into one string
    /// made a missing harness and a deliberate abort read identically at every
    /// call site above.
    fn from(err: RunError) -> Self {
        match err {
            RunError::Timeout => Self::DispatchTimeout,
            RunError::Aborted => Self::DispatchAborted,
            RunError::Worker(message) => Self::Harness(message),
            // A failed node's transcript is the step's own account — the
            // workflow error that surfaces the failure carries only the message.
            RunError::WorkerWithTranscript { message, .. } => Self::Harness(message),
            RunError::Busy(message) => Self::Unreachable(message),
            // Same shape as backpressure from a workflow's point of view: the
            // harness exists and is fine, it simply cannot be reached for this
            // run. A workflow has no operator to hand anything back to.
            RunError::Held(message) => Self::Unreachable(message),
            RunError::Transport(message) => Self::Unreachable(message),
        }
    }
}
