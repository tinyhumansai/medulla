//! Claude Code hook delivery: the shared hook document, wrapped in the inline
//! settings object `--settings` layers over the user's `settings.json`.
//!
//! Verified against Claude Code 2.1.221: a `SessionStart` command hook passed
//! this way runs, and the user's own `settings.json` hooks keep running
//! alongside it.

use serde_json::{json, Value};

/// The `--settings` argument pair carrying `document` as Claude Code's `hooks`
/// setting.
///
/// Built with `serde_json` rather than string interpolation so an operator's
/// shell command — quotes and backslashes included — can never break the JSON.
///
/// Callers applying commit attribution as well must go through
/// [`super::launch_args`]: Claude Code takes one `--settings`, and a second
/// occurrence replaces rather than merges.
pub fn settings_args(document: &Value) -> Vec<String> {
    let settings = json!({ "hooks": document });
    vec!["--settings".to_string(), settings.to_string()]
}
