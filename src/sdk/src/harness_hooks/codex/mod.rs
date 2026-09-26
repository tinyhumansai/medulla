//! Codex hook delivery: the shared hook document as a `-c hooks=…` config
//! override, plus the trust-store edit that lets Codex actually run it.
//!
//! Verified against Codex 0.146.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value;

use super::native::to_inline_toml;
use super::types::HookEvent;

pub mod trust;

/// The argv entries carrying `document` as Codex's `hooks` config override.
///
/// Codex parses `-c key=value` as TOML, and a command line cannot carry a
/// multi-line table, so the document is encoded as one inline table. Codex
/// merges config layers additively for hooks, so this adds to the operator's own
/// `~/.codex/hooks.json` rather than replacing it.
///
/// Pure, and separate from [`trust_injected`] because delivery is what the Hooks
/// page wants to describe and trust is what only a real spawn should do.
pub fn config_args(document: &Value) -> Vec<String> {
    vec![
        "-c".to_string(),
        format!("hooks={}", to_inline_toml(document)),
    ]
}

/// Approve this spawn's injected hooks in Codex's trust store, as
/// operator-facing notes.
///
/// Empty when everything was already trusted, which is the steady state and not
/// worth a line in a log that fires on every spawn. The two cases that do
/// produce one are the first launch after a hook changed — where Codex has not
/// registered it yet and it will start running on the next launch — and a
/// trust store that could not be written, where saying nothing would leave
/// hooks silently not running, which is the failure this whole module exists to
/// make impossible.
pub fn trust_injected(events: &[HookEvent], env: &HashMap<String, String>) -> Vec<String> {
    let config_path = config_path(env);
    match trust::enable(&config_path, events) {
        Ok(0) => Vec::new(),
        Ok(count) => vec![format!(
            "enabled {count} Medulla hook(s) in Codex's trust store; a hook Codex is seeing for \
             the first time starts running on the next launch"
        )],
        Err(error) => vec![format!(
            "could not enable Medulla's hooks in {}: {error}. Codex skips hooks it has not been \
             told to trust, so they will not run until you accept them on its Hooks screen.",
            config_path.display()
        )],
    }
}

/// The Codex config file holding the hook trust store: `$CODEX_HOME/config.toml`,
/// else `~/.codex/config.toml`.
///
/// Read from the child's own environment rather than this process's, so a spawn
/// pointed at a scratch `CODEX_HOME` — a test, a sandboxed worker — has its
/// trust decided in that home and never touches the operator's.
fn config_path(env: &HashMap<String, String>) -> PathBuf {
    let home = env
        .get("CODEX_HOME")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir(env).join(".codex"));
    home.join("config.toml")
}

/// The child's home directory, from its own environment.
fn home_dir(env: &HashMap<String, String>) -> PathBuf {
    for key in ["HOME", "USERPROFILE"] {
        if let Some(value) = env
            .get(key)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            return PathBuf::from(value);
        }
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}
