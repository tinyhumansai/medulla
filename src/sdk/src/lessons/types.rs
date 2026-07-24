//! Data types for the workspace lessons ledger.

use std::io;
use std::path::PathBuf;

/// One durable operator lesson stored in a workspace profile.
///
/// Each lesson is a condition-action pair: when the named situation arises
/// (the [`trigger`](Self::trigger)), the operator should take the prescribed
/// action (the [`rule`](Self::rule)). Lessons are stored as plain Markdown
/// list items under the `## Lessons` heading in `MEDULLA.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lesson {
    /// The condition or situation the lesson is about ("CI flakes",
    /// "submitting a PR", "a test times out").
    pub trigger: String,
    /// The prescribed action or advice ("rerun the focused test",
    /// "squash the fixup commits").
    pub rule: String,
}

impl Lesson {
    /// Create a new lesson from any owned trigger/rule strings.
    ///
    /// The returned value is not yet validated: callers must pass it through
    /// [`super::normalize`] (or one of the public entry points that calls it)
    /// before persisting.  `new` itself never fails.
    pub fn new(trigger: impl Into<String>, rule: impl Into<String>) -> Self {
        Self {
            trigger: trigger.into(),
            rule: rule.into(),
        }
    }
}

/// Whether an append changed the profile on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddLessonOutcome {
    /// The lesson was appended successfully.
    Added,
    /// An identical normalized lesson already exists; the profile was left as-is.
    AlreadyPresent,
}

/// A typed failure while reading or updating a lessons ledger.
#[derive(Debug, thiserror::Error)]
pub enum LessonError {
    /// The workspace profile file (`MEDULLA.md`) does not exist at the given path.
    #[error("workspace profile not found at {0}")]
    MissingProfile(PathBuf),
    /// One or both of the trigger and rule are empty or whitespace-only.
    #[error("lesson trigger and rule must both be non-empty")]
    EmptyLesson,
    /// The string did not contain the `->` delimiter required by the CLI and
    /// slash-command syntax.
    #[error("expected a lesson in the form <trigger> -> <rule>")]
    InvalidLessonFormat,
    /// The trigger contains `: `, or the trigger or rule contains `->`, which
    /// are reserved as the CLI and on-disk delimiters and cannot appear
    /// literally in those fields.
    #[error("lesson trigger must not contain ': ' delimiter; neither field may contain '->'")]
    DelimiterInField,
    /// The trigger or rule contains an embedded line break and cannot be stored
    /// safely in the single-line `- when …: …` format.
    #[error("lesson trigger and rule must not contain line breaks")]
    MultilineField,
    /// An I/O operation on the workspace profile failed.
    #[error("failed to read or write workspace profile {path}: {source}")]
    Io {
        /// The path of the profile being read or written.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },
}
