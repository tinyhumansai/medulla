//! Test-only data types for workspace initialisation.

/// A provider that returns a canned response or error.
pub(super) struct StubProvider(pub(super) Result<String, String>);
