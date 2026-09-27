//! Data types for crash reporting: the process guard, the resolved status, and
//! the `sentry-test` diagnostic's outcome.

use std::fmt;

use sentry::types::Uuid;

/// Why crash reporting is (or is not) running in this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashReportingStatus {
    /// [`init`](super::init) has not run in this process.
    Uninitialized,
    /// Reports are being sent.
    Active,
    /// The operator opted out through
    /// [`DISABLED_ENV`](super::DISABLED_ENV).
    Disabled,
    /// A DSN was supplied but did not parse.
    InvalidDsn,
}

impl fmt::Display for CrashReportingStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Uninitialized => "crash reporting was not initialized in this process",
            Self::Active => "crash reporting is active",
            Self::Disabled => "crash reporting is disabled by MEDULLA_ANALYTICS_DISABLED",
            Self::InvalidDsn => "crash reporting is inactive: the Sentry DSN does not parse",
        })
    }
}

/// Keeps the Sentry client alive; dropping it flushes queued reports.
///
/// Hold it for the life of the process (bind it in `main`, not `_`). It is
/// inert — and dropping it free — when crash reporting is not active.
#[must_use = "dropping the guard immediately shuts crash reporting down"]
pub struct CrashReportingGuard {
    pub(super) client: Option<sentry::ClientInitGuard>,
}

impl CrashReportingGuard {
    /// Whether this guard holds a live Sentry client.
    pub fn is_active(&self) -> bool {
        self.client.as_ref().is_some_and(|guard| guard.is_enabled())
    }
}

/// What the `sentry-test` diagnostic observed after sending its event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestEventReport {
    /// The id Sentry will show the event under.
    pub event_id: Uuid,
    /// Whether the transport drained its queue within the timeout.
    pub flushed: bool,
    /// The ingestion endpoint's HTTP status for the last delivery, when a
    /// response arrived. `200` means Sentry accepted the event.
    pub http_status: Option<u16>,
}
