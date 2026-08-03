//! Git commit attribution for Medulla-launched harnesses.
//!
//! When Medulla spawns a coding-agent CLI, commits that agent makes should say
//! so. This module builds the flags and environment that carry that
//! attribution, as pure functions over a resolved on/off value, so the wrapper
//! and the headless daemon behave identically and both are unit-testable.
//!
//! # Configuration
//!
//! Attribution is config-driven: the `[attribution] commit` config key
//! ([`crate::config::AttributionConfig`]), on by default.
//! Callers resolve it from the loaded config and pass it in — this module never
//! reads the environment or the filesystem to decide.
//!
//! # Mechanism
//!
//! Attribution is a `Co-authored-by` trailer on the commit *message*, not a
//! change of git author or committer identity. The human who ran the session
//! stays the author, so blame, `git log --author`, and the GitHub contribution
//! graph are unaffected.
//!
//! The trailer is injected per-spawn via the harness's own CLI flags, never by
//! writing config files. Nothing is persisted: a user's own `claude` sessions
//! keep whatever attribution setting they configured, and only harnesses that
//! Medulla launches carry the Medulla trailer.
//!
//! # Coverage
//!
//! Every provider is attributed the same way: a temporary `prepare-commit-msg`
//! git hook ([`prepare_commit_msg`]) that runs inside `git commit` itself.
//!
//! This is deliberate. Claude Code does expose an `attribution.commit` setting,
//! and `--settings` layers inline JSON over the user's `settings.json`, but the
//! setting is only *advisory*: its value is interpolated into the Bash tool
//! description as "End git commit messages with: …", so it depends on the model
//! choosing to follow it. Verified against Claude Code 2.1.220 — when the task
//! brief dictates the commit message ("commit with message 'x'"), the model
//! writes that message verbatim and the trailer never appears. Since a Medulla
//! task brief routinely dictates commit messages, that path dropped attribution
//! exactly when it mattered.
//!
//! Codex (as of 0.144.6) hardcodes `Co-authored-by: Codex <noreply@openai.com>`
//! with no config key to override it, and Opencode exposes no equivalent knob
//! at all.
//!
//! So the hook is the mechanism of record. Claude additionally still receives
//! the `--settings` flag ([`attribution_args`]) as a belt-and-braces hint; the
//! hook applies the trailer with `git interpret-trailers --if-exists
//! addIfDifferent`, so the two mechanisms agreeing produces one trailer, not two.

use std::collections::HashMap;

use crate::protocol::HarnessProvider;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod prepare_commit_msg_tests;

/// Generator for the git hook shims that inject the Medulla `Co-authored-by`
/// trailer via environment variables. The mechanism of record for every
/// provider.
pub mod prepare_commit_msg;

/// Display name used in the `Co-authored-by` trailer.
pub const ATTRIBUTION_NAME: &str = "Medulla";

/// Email used in the `Co-authored-by` trailer. Registered on the
/// <https://github.com/medullabot> account so the trailer links to that profile.
pub const ATTRIBUTION_EMAIL: &str = "medulla@tinyhumans.ai";

/// The trailer line appended to commit messages, e.g.
/// `Co-authored-by: Medulla <medulla@tinyhumans.ai>`.
///
/// GitHub requires a blank line between the commit body and this trailer; the
/// harness composing the message is responsible for that separation.
pub fn attribution_trailer() -> String {
    format!("Co-authored-by: {ATTRIBUTION_NAME} <{ATTRIBUTION_EMAIL}>")
}

/// Extra CLI arguments that make `provider` attribute its commits to Medulla.
///
/// `enabled` is the resolved `attribution.commit` config value — see
/// [`crate::config::AttributionConfig`]. Returns an empty vector when
/// attribution is off, or when the provider has no mechanism to retarget its
/// trailer (Codex, Opencode). Callers prepend these to the child argv alongside
/// `TINYPLACE_<P>_ARGS`.
pub fn attribution_args(provider: HarnessProvider, enabled: bool) -> Vec<String> {
    if !enabled {
        return Vec::new();
    }
    match provider {
        HarnessProvider::Claude => vec!["--settings".to_string(), claude_settings_json()],
        // No override exists for these; see the module docs.
        HarnessProvider::Codex | HarnessProvider::Opencode => Vec::new(),
    }
}

/// The inline JSON handed to `claude --settings`, layering only
/// `attribution.commit` over the user's own settings.
///
/// Built with `serde_json` rather than string interpolation so the identity can
/// never break the JSON encoding.
fn claude_settings_json() -> String {
    let value = serde_json::json!({
        "attribution": { "commit": attribution_trailer() },
    });
    value.to_string()
}

/// Environment variables that make a harness attribute its commits to Medulla
/// via the git hook shims.
///
/// Every provider takes this path, including Claude Code — see the module docs
/// for why its own `attribution.commit` setting is not sufficient on its own.
/// The hook is gated at runtime by `MEDULLA_ATTRIBUTION`.
///
/// `enabled` is the resolved `attribution.commit` config value — see
/// [`crate::config::AttributionConfig`].
///
/// The hook directory is shared process-wide and outlives every child, so this
/// is safe to call for concurrent spawns and needs no cleanup from the caller
/// (see [`prepare_commit_msg`]).
///
/// `base_env` is the environment the result will be merged into: the returned
/// `GIT_CONFIG_*` entries are appended after whatever it already carries rather
/// than replacing them.
///
/// Returns an empty map when attribution is off, and on non-Unix platforms
/// (git hooks are not supported there).
pub fn attribution_env(
    enabled: bool,
    base_env: &HashMap<String, String>,
) -> HashMap<String, String> {
    if !enabled {
        return HashMap::new();
    }
    prepare_commit_msg::hook_env(&attribution_trailer(), base_env)
}
