//! Git commit attribution for Medulla-launched harnesses.
//!
//! When Medulla spawns a coding-agent CLI, commits that agent makes should say
//! so. This module resolves the CLI flags that carry that attribution, as pure
//! functions over an injected environment map — the same contract as
//! [`super::env`], so precedence is unit-testable and identical across the
//! wrapper and the headless daemon.
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
//! Only Claude Code exposes a knob for this today. Its `attribution.commit`
//! setting *replaces* the built-in `Co-Authored-By: Claude <noreply@anthropic.com>`
//! line verbatim, and `--settings` accepts inline JSON that layers over the
//! user's `settings.json`.
//!
//! Codex (as of 0.144.6) hardcodes `Co-authored-by: Codex <noreply@openai.com>`
//! with no config key to override it, and Opencode exposes no equivalent. Both
//! therefore resolve to no arguments — they are left to their own defaults
//! rather than silently misattributed.

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

/// Kill-switch env var. Any value other than `1` / `true` / `yes` / `on`
/// disables attribution.
pub const ATTRIBUTION_ENV_KEY: &str = "TINYPLACE_GIT_ATTRIBUTION";

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

/// Whether attribution is enabled. Defaults to `true`; set
/// `TINYPLACE_GIT_ATTRIBUTION` to `0` / `false` / `no` / `off` (or empty) to
/// disable. Unrecognised values are treated as disabled, so a typo fails closed
/// rather than silently attributing.
pub fn attribution_enabled(env: &HashMap<String, String>) -> bool {
    match env.get(ATTRIBUTION_ENV_KEY) {
        None => true,
        Some(raw) => matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
    }
}

/// Extra CLI arguments that make `provider` attribute its commits to Medulla.
///
/// Returns an empty vector when attribution is disabled, or when the provider
/// has no mechanism to retarget its trailer (Codex, Opencode). Callers prepend
/// these to the child argv alongside `TINYPLACE_<P>_ARGS`.
pub fn attribution_args(provider: HarnessProvider, env: &HashMap<String, String>) -> Vec<String> {
    if !attribution_enabled(env) {
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
/// Claude Code receives CLI flags instead ([`attribution_args`]), so this
/// returns empty for it. Codex and Opencode get their attribution through a
/// temporary `prepare-commit-msg` hook gated by `MEDULLA_ATTRIBUTION`.
///
/// The hook directory is stored in module-level state and must be cleaned up
/// after the harness exits by calling [`cleanup_hook_tmpdir`].
///
/// Returns an empty map when attribution is disabled for *any* provider.
pub fn attribution_env(
    provider: HarnessProvider,
    env: &HashMap<String, String>,
) -> HashMap<String, String> {
    if !attribution_enabled(env) {
        return HashMap::new();
    }
    match provider {
        HarnessProvider::Claude => HashMap::new(),
        HarnessProvider::Codex | HarnessProvider::Opencode => {
            let (hook_env, hook_dir) = prepare_commit_msg::generate_hook(&attribution_trailer());
            *HOOK_DIR.lock().unwrap() = Some(hook_dir);
            hook_env
        }
    }
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
