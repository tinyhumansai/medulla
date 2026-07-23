//! Data types for the workspace lessons ledger.

use std::io;
use std::path::PathBuf;

/// One durable operator lesson stored in a workspace profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lesson {
    pub trigger: String,
    pub rule: String,
}

impl Lesson {
    pub fn new(trigger: impl Into<String>, rule: impl Into<String>) -> Self {
        Self {
            trigger: trigger.into(),
            rule: rule.into(),
        }
    }
}

/// Whether an append changed the profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddLessonOutcome {
    Added,
    AlreadyPresent,
}

/// A typed failure while reading or updating a lessons ledger.
#[derive(Debug, thiserror::Error)]
pub enum LessonError {
    #[error("workspace profile not found at {0}")]
    MissingProfile(PathBuf),
    #[error("lesson trigger and rule must both be non-empty")]
    EmptyLesson,
    #[error("expected a lesson in the form <trigger> -> <rule>")]
    InvalidLessonFormat,
    #[error("failed to read or write workspace profile {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}
