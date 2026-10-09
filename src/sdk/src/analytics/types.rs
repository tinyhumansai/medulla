//! Status and error types for product analytics.

use std::fmt;

/// Why product analytics is (or is not) sending events in this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalyticsStatus {
    /// Events are being sent.
    Active,
    /// The operator opted out through
    /// [`DISABLED_ENV`](crate::observability::DISABLED_ENV).
    Disabled,
    /// A configured value is not a legal HTTP header value, or the HTTP client
    /// could not be built.
    InvalidConfig,
}

impl fmt::Display for AnalyticsStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Active => "analytics is active",
            Self::Disabled => "analytics is disabled by MEDULLA_ANALYTICS_DISABLED",
            Self::InvalidConfig => {
                "analytics is inactive: the build's OpenPanel configuration is not usable"
            }
        })
    }
}

/// A failed OpenPanel request.
///
/// Analytics callers treat this as non-fatal; the type exists so the transport
/// stays testable and the `analytics-test` diagnostic can say what went wrong.
#[derive(Debug, thiserror::Error)]
pub enum AnalyticsError {
    /// Analytics is not active, so nothing was sent.
    #[error("{0}")]
    Inactive(AnalyticsStatus),
    /// The request did not complete.
    #[error("OpenPanel request failed: {0}")]
    Request(#[from] reqwest::Error),
    /// The account id is not an account id's shape, so nothing was sent.
    #[error("not an account id; nothing was sent")]
    InvalidAccountId,
    /// OpenPanel answered with a non-success status.
    #[error("OpenPanel rejected the event with HTTP {0}")]
    Rejected(u16),
}
