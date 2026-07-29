//! Wire models for the worker roster and operator-owned task program APIs.
//!
//! These now live in `tinyhumans-sdk` and are re-exported here under this
//! crate's established names, so the shared SDK owns one definition of the
//! contract while callers in this repo keep the names they already use.
//!
//! The SDK names are namespace-scoped (`medulla::Task`), which would read as
//! ambiguous once flattened into this crate's `client::*` surface — hence the
//! `Program*` prefixes retained below.

pub use tinyhumans_sdk::api::medulla::{
    GithubIssueState, Roster, RosterBudget, RosterWorker, TaskRecurrence, TaskRecurrenceFrequency,
    TaskSourceSyncResult,
};

/// An operator-owned task in the backend program ledger.
pub use tinyhumans_sdk::api::medulla::Task as ProgramTask;
/// A configured GitHub task source. Tokens never appear on this response.
pub use tinyhumans_sdk::api::medulla::TaskSource as ProgramTaskSource;
/// External source identity attached to a synchronized task.
pub use tinyhumans_sdk::api::medulla::TaskSourceRef as ProgramTaskSourceRef;
/// Lifecycle state of an operator-owned program task.
pub use tinyhumans_sdk::api::medulla::TaskStatus as ProgramTaskStatus;

/// Input for creating a program task.
pub use tinyhumans_sdk::api::medulla::CreateTaskRequest as CreateProgramTask;
/// Input for configuring a GitHub task source.
pub use tinyhumans_sdk::api::medulla::CreateTaskSourceRequest as CreateProgramTaskSource;
/// Patch accepted by `PATCH /medulla/v1/tasks/:id`.
///
/// `recurrence` is doubly-optional: outer `None` omits the field, inner `None`
/// sends JSON `null` to clear the rule, and `Some(Some(_))` replaces it.
pub use tinyhumans_sdk::api::medulla::UpdateTaskRequest as UpdateProgramTask;
