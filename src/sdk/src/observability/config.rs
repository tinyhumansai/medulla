//! Resolution of the crash-reporting opt-out, DSN, release, and environment.
//!
//! Pure functions over their inputs, so the precedence rules are testable
//! without mutating the process environment.

/// Pick the DSN: a non-blank runtime value wins over the compiled-in one.
///
/// Blank counts as absent on both sides — CI exports an empty variable when
/// the repository variable is unset, and an operator clearing the variable
/// should fall back to the build's DSN rather than break it.
pub(super) fn resolve_dsn(runtime: Option<&str>, compiled: Option<&str>) -> Option<String> {
    [runtime, compiled]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(str::to_owned)
}

/// The Sentry release every report is filed under: `medulla@<version>`.
pub(super) fn release() -> String {
    format!("medulla@{}", env!("CARGO_PKG_VERSION"))
}

/// The Sentry environment: an explicit non-blank override, else `production`
/// for an optimized build and `development` for a debug one.
pub(super) fn resolve_environment(runtime: Option<&str>, debug_build: bool) -> String {
    match runtime.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => value.to_ascii_lowercase(),
        None if debug_build => "development".to_owned(),
        None => "production".to_owned(),
    }
}

/// Whether the opt-out variable's value turns reporting off: `1`, `true`, or
/// `yes`, case-insensitively and ignoring surrounding whitespace.
pub(super) fn is_opted_out(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes"
        )
    })
}
