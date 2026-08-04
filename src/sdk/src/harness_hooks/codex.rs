//! Codex hook delivery: the shared hook document as a `-c hooks=…` config
//! override.
//!
//! Verified against Codex 0.146.

use serde_json::Value;

use super::native::to_inline_toml;

/// The argv entries carrying `document` as Codex's `hooks` config override.
///
/// Codex parses `-c key=value` as TOML, and a command line cannot carry a
/// multi-line table, so the document is encoded as one inline table. Codex
/// merges config layers additively for hooks, so this adds to the operator's own
/// `~/.codex/hooks.json` rather than replacing it.
pub fn config_args(document: &Value) -> Vec<String> {
    vec![
        "-c".to_string(),
        format!("hooks={}", to_inline_toml(document)),
    ]
}
