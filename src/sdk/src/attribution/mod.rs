//! Git commit attribution for Medulla-launched harnesses.
//!
//! When Medulla spawns a coding-agent CLI, commits that agent makes should say
//! so. This module builds the flags and environment that carry that
//! attribution, as pure functions over a resolved on/off value, so the wrapper
//! and the headless daemon behave identically and both are unit-testable.
//!
//! # Configuration
//!
//! Attribution is config-driven: the `attribution.commit` key of
//! `medulla.tui.json` ([`crate::config::AttributionConfig`]), on by default.
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
use std::path::PathBuf;
use std::sync::Mutex;

use crate::tinyplace::HarnessProvider;

#[cfg(test)]
mod tests;

/// Generator for `prepare-commit-msg` git hooks that inject the Medulla
/// `Co-authored-by` trailer via environment variables. Used for providers
/// (Codex, Opencode) whose CLI has no built-in attribution knob.
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

/// Module-level storage for the temporary hook directory, so
/// [`cleanup_hook_tmpdir`] can remove it after the harness exits without the
/// caller needing to carry a [`PathBuf`] through every spawn path.
static HOOK_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Environment variables that make a harness attribute its commits to Medulla
/// via the `prepare-commit-msg` git hook.
///
/// Every provider takes this path, including Claude Code — see the module docs
/// for why its own `attribution.commit` setting is not sufficient on its own.
/// The hook is gated at runtime by `MEDULLA_ATTRIBUTION`.
///
/// `enabled` is the resolved `attribution.commit` config value — see
/// [`crate::config::AttributionConfig`].
///
/// The hook directory is stored in module-level state and must be cleaned up
/// after the harness exits by calling [`cleanup_hook_tmpdir`]. A previously
/// generated directory is cleaned up here rather than leaked, so repeated
/// spawns in one process do not accumulate temp directories.
///
/// Returns an empty map when attribution is off, and on non-Unix platforms
/// (git hooks are not supported there).
pub fn attribution_env(enabled: bool) -> HashMap<String, String> {
    if !enabled {
        return HashMap::new();
    }
    let (hook_env, hook_dir) = prepare_commit_msg::generate_hook(&attribution_trailer());
    if let Some(stale) = HOOK_DIR.lock().unwrap().replace(hook_dir) {
        prepare_commit_msg::cleanup_hook_dir(&stale);
    }
    hook_env
}

/// Remove the temporary hook directory that [`attribution_env`] created.
///
/// Safe to call even when no hook was generated — this is a no-op in that
/// case. Idempotent: a second call after cleanup does nothing.
pub fn cleanup_hook_tmpdir() {
    let mut guard = HOOK_DIR.lock().unwrap();
    if let Some(path) = guard.take() {
        prepare_commit_msg::cleanup_hook_dir(&path);
    }
}
