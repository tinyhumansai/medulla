//! Codex hook delivery: the shared hook document as a `-c hooks=…` config
//! override, plus the trust bypass that override requires.
//!
//! Verified against Codex 0.146.

use serde_json::Value;

use super::native::to_inline_toml;

/// Codex's per-invocation escape from the hook trust store.
///
/// Codex records trust against a hook's content hash and skips hooks it has not
/// seen before — quietly, with nothing on stderr. A per-spawn injection is by
/// construction absent from the operator's trust store, so without this flag
/// every Medulla hook would be silently dropped. Codex documents the flag as
/// intended for "automation that already vets hook sources", which is exactly
/// this: the hooks come from the operator's own Medulla config, not from the
/// workspace or the model.
const BYPASS_HOOK_TRUST: &str = "--dangerously-bypass-hook-trust";

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
        BYPASS_HOOK_TRUST.to_string(),
    ]
}
