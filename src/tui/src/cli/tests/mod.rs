//! Unit tests for the CLI plumbing, split by the surface under test:
//! [`subcommands`] covers dispatch and the long-standing flag parsers,
//! [`skills`] the `medulla skills` parser. Both share the `argv` helper below.

#[cfg(feature = "workflows")]
mod skills;
mod subcommands;

/// Build an owned arg vector the way `main` hands one to the parsers.
fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}
